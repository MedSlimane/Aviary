import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent } from "@tauri-apps/plugin-updater";

export type UpdateMetadata = {
  currentVersion: string;
  version: string;
  notes: string | null;
  publishedAt: string | null;
};

export type UpdateCheckResult =
  | { status: "disabled-in-development"; currentVersion: string }
  | { status: "current"; currentVersion: string }
  | ({ status: "available" } & UpdateMetadata)
  | { status: "error"; currentVersion: string | null; message: string };

export type UpdateProgress =
  | { event: "started"; contentLength?: number }
  | {
      event: "progress";
      chunkLength: number;
      downloadedBytes: number;
      contentLength?: number;
    }
  | {
      event: "finished";
      downloadedBytes: number;
      contentLength?: number;
    };

export type UpdateInstallResult =
  | { status: "disabled-in-development"; currentVersion: string }
  | { status: "current"; currentVersion: string }
  | ({ status: "changed" } & UpdateMetadata)
  | ({ status: "installed" } & UpdateMetadata)
  | ({ status: "installed-relaunch-required"; message: string } & UpdateMetadata)
  | {
      status: "error";
      currentVersion: string | null;
      phase: "check" | "download-install";
      message: string;
    };

type InstallOptions = {
  expectedVersion?: string;
  relaunchAfterInstall?: boolean;
  onProgress?: (progress: UpdateProgress) => void;
};

let launchCheck: Promise<UpdateCheckResult> | undefined;

// The plugin applies these to the underlying HTTP requests. A failed network
// path must return control to the UI instead of leaving a release build in an
// indefinite checking or installing state.
const CHECK_TIMEOUT_MS = 20_000;
const DOWNLOAD_TIMEOUT_MS = 10 * 60_000;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Returns an error for UI recovery when the platform cannot relaunch. */
export async function requestRelaunch(): Promise<string | null> {
  try {
    await relaunch();
    return null;
  } catch (error) {
    return errorMessage(error);
  }
}

function metadata(
  currentVersion: string,
  update: {
    version: string;
    body?: string;
    date?: string;
  },
): UpdateMetadata {
  return {
    currentVersion,
    version: update.version,
    notes: update.body?.trim() || null,
    publishedAt: update.date || null,
  };
}

/**
 * Checks the signed release feed and reports only states observed from the
 * updater. Development builds never contact the production update channel.
 */
export async function checkForUpdate(): Promise<UpdateCheckResult> {
  let currentVersion: string;
  try {
    currentVersion = await getVersion();
  } catch (error) {
    return { status: "error", currentVersion: null, message: errorMessage(error) };
  }

  if (import.meta.env.DEV) {
    return { status: "disabled-in-development", currentVersion };
  }

  try {
    const update = await check({ timeout: CHECK_TIMEOUT_MS });
    if (!update) return { status: "current", currentVersion };

    try {
      return { status: "available", ...metadata(currentVersion, update) };
    } finally {
      await update.close().catch(() => {});
    }
  } catch (error) {
    return { status: "error", currentVersion, message: errorMessage(error) };
  }
}

/** Deduplicates React StrictMode/startup calls while keeping manual checks fresh. */
export function checkForUpdateOnce(): Promise<UpdateCheckResult> {
  launchCheck ??= checkForUpdate();
  return launchCheck;
}

/**
 * Re-checks immediately before installation. If the feed changed after the
 * user accepted a prompt, `changed` is returned so the new version can be
 * shown and confirmed instead of silently installing a different release.
 */
export async function installAvailableUpdate(
  options: InstallOptions = {},
): Promise<UpdateInstallResult> {
  let currentVersion: string;
  try {
    currentVersion = await getVersion();
  } catch (error) {
    return {
      status: "error",
      currentVersion: null,
      phase: "check",
      message: errorMessage(error),
    };
  }

  if (import.meta.env.DEV) {
    return { status: "disabled-in-development", currentVersion };
  }

  let update;
  try {
    update = await check({ timeout: CHECK_TIMEOUT_MS });
  } catch (error) {
    return {
      status: "error",
      currentVersion,
      phase: "check",
      message: errorMessage(error),
    };
  }

  if (!update) return { status: "current", currentVersion };

  const found = metadata(currentVersion, update);
  if (options.expectedVersion && options.expectedVersion !== update.version) {
    await update.close().catch(() => {});
    return { status: "changed", ...found };
  }

  let downloadedBytes = 0;
  let contentLength: number | undefined;

  try {
    await update.downloadAndInstall(
      (event: DownloadEvent) => {
        if (event.event === "Started") {
          contentLength = event.data.contentLength ?? undefined;
          options.onProgress?.({ event: "started", contentLength });
          return;
        }

        if (event.event === "Progress") {
          downloadedBytes += event.data.chunkLength;
          options.onProgress?.({
            event: "progress",
            chunkLength: event.data.chunkLength,
            downloadedBytes,
            contentLength,
          });
          return;
        }

        options.onProgress?.({
          event: "finished",
          downloadedBytes,
          contentLength,
        });
      },
      { timeout: DOWNLOAD_TIMEOUT_MS },
    );
  } catch (error) {
    return {
      status: "error",
      currentVersion,
      phase: "download-install",
      message: errorMessage(error),
    };
  } finally {
    await update.close().catch(() => {});
  }

  if (options.relaunchAfterInstall === false) {
    return { status: "installed", ...found };
  }

  const relaunchError = await requestRelaunch();
  if (relaunchError) {
    return {
      status: "installed-relaunch-required",
      ...found,
      message: relaunchError,
    };
  }
  return { status: "installed", ...found };
}
