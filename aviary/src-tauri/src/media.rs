//! The media board.
//!
//! Media is **content-addressed**: a file's sha256 is its identity, and the
//! bytes are copied into `~/.aviary/media/<hash>/original.<ext>`. Three things
//! follow from that, and they are the reason for the design:
//!
//! * Re-importing the same image is a no-op, not a duplicate tile — dedupe is
//!   free rather than a perceptual-hash pass.
//! * A tile never dies because the original was moved, renamed, or cleaned out
//!   of `~/Downloads`. The original path is kept only as provenance.
//! * `aviary-media` can hand any agent a stable path that will still resolve
//!   tomorrow.
//!
//! Thumbnails live under `~/.aviary/cache/thumbs` because they are derivable;
//! deleting the cache costs a regeneration and nothing else.

use crate::store;
use image::GenericImageView;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const THUMB_SIZE: u32 = 640;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub hash: String,
    pub kind: String,
    pub ext: String,
    pub bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<String>,
    /// Dominant colour as `#rrggbb`, used for the colour filter and as the
    /// tile's placeholder while the thumbnail decodes.
    pub dominant: Option<String>,
    pub origin: Option<String>,
    pub title: Option<String>,
    pub note: Option<String>,
    pub added_at: i64,
    pub tags: Vec<String>,
    /// Absolute path to the stored original.
    pub path: String,
    /// Absolute path to the cached thumbnail, when one exists.
    pub thumb: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
        .to_lowercase()
}

/// Images we can decode (and therefore thumbnail and colour-sample) versus
/// everything else, which is still stored and searchable but not analysed.
fn kind_of(ext: &str) -> &'static str {
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" => "image",
        "mp4" | "mov" | "webm" | "m4v" => "video",
        _ => "file",
    }
}

pub fn store_path(hash: &str, ext: &str) -> Option<PathBuf> {
    store::media_dir().map(|d| d.join(hash).join(format!("original.{ext}")))
}

fn thumb_path(hash: &str) -> Option<PathBuf> {
    store::thumb_dir().map(|d| d.join(format!("{hash}@{THUMB_SIZE}.jpg")))
}

/// Averages in linear-ish space over a downscaled copy.
///
/// Deliberately an average rather than a k-means "dominant" colour: it is
/// stable, cheap, and for the gradient-and-screenshot material this board holds
/// it produces the tint a designer would actually name. A true modal colour
/// tends to return the background of a screenshot, which is less useful.
fn average_colour(img: &image::DynamicImage) -> String {
    let small = img.thumbnail(64, 64).to_rgb8();
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for px in small.pixels() {
        r += (px[0] as u64).pow(2);
        g += (px[1] as u64).pow(2);
        b += (px[2] as u64).pow(2);
        n += 1;
    }
    if n == 0 {
        return "#000000".into();
    }
    // Root-mean-square keeps saturated colours from washing toward grey the way
    // a plain mean does.
    let f = |sum: u64| ((sum / n) as f64).sqrt().round().clamp(0.0, 255.0) as u8;
    format!("{:02x}", f(r))
        .chars()
        .chain(format!("{:02x}", f(g)).chars())
        .chain(format!("{:02x}", f(b)).chars())
        .fold(String::from("#"), |mut acc, c| {
            acc.push(c);
            acc
        })
}

/// Names a hue coarsely enough to be a useful filter chip.
fn colour_name(hex: &str) -> Option<&'static str> {
    let v = u32::from_str_radix(hex.strip_prefix('#')?, 16).ok()?;
    let (r, g, b) = (
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    if max < 0.16 {
        return Some("dark");
    }
    if delta < 0.08 {
        return Some(if max > 0.75 { "light" } else { "grey" });
    }

    let hue = 60.0
        * if max == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };

    // Boundaries are tuned to what a designer would *call* the colour, not to
    // even 30° slices. Aviary's own `accent/violet` (#8D7AE8) sits at hue 250,
    // which a naive split files under blue — so violet starts at 245. Likewise
    // `dusk` rose (#B34E78) is 335, magenta rather than red.
    Some(match hue {
        h if h < 15.0 || h >= 345.0 => "red",
        h if h < 45.0 => "orange",
        h if h < 70.0 => "gold",
        h if h < 165.0 => "green",
        h if h < 200.0 => "teal",
        h if h < 245.0 => "blue",
        h if h < 290.0 => "violet",
        _ => "magenta",
    })
}

/// Imports one file. Returns the item, whether it already existed or not.
pub fn import(source: &Path) -> Result<MediaItem, String> {
    let bytes = std::fs::read(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let ext = ext_of(source);
    let kind = kind_of(&ext);

    // Already known: return it rather than writing a second copy.
    if let Some(existing) = get(&hash) {
        return Ok(existing);
    }

    let dest = store_path(&hash, &ext).ok_or("no home directory")?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;

    let mut width = None;
    let mut height = None;
    let mut orientation = None;
    let mut dominant = None;
    let mut auto_tags: Vec<String> = vec![kind.to_string()];

    if kind == "image" {
        if let Ok(img) = image::load_from_memory(&bytes) {
            let (w, h) = img.dimensions();
            width = Some(w);
            height = Some(h);
            let o = if w > h {
                "landscape"
            } else if h > w {
                "portrait"
            } else {
                "square"
            };
            orientation = Some(o.to_string());
            auto_tags.push(o.to_string());

            let hex = average_colour(&img);
            if let Some(name) = colour_name(&hex) {
                auto_tags.push(name.to_string());
            }
            dominant = Some(hex);

            // Thumbnail now, so the board never decodes full-size originals.
            if let Some(tp) = thumb_path(&hash) {
                if let Some(parent) = tp.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let thumb = img.thumbnail(THUMB_SIZE, THUMB_SIZE);
                if thumb.to_rgb8().save_with_format(&tp, image::ImageFormat::Jpeg).is_ok() {
                    let _ = store::cache().execute(
                        "INSERT OR REPLACE INTO thumb(hash, size, path) VALUES (?1, ?2, ?3)",
                        params![hash, THUMB_SIZE as i64, tp.to_string_lossy()],
                    );
                }
            }
        }
    }

    let title = source
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());

    {
        let conn = store::data();
        conn.execute(
            "INSERT INTO media(hash, kind, ext, bytes, width, height, orientation,
                               dominant, origin, title, added_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                hash,
                kind,
                ext,
                bytes.len() as i64,
                width,
                height,
                orientation,
                dominant,
                source.to_string_lossy(),
                title,
                store::now()
            ],
        )
        .map_err(|e| e.to_string())?;

        for t in &auto_tags {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO tag(media_hash, tag, auto) VALUES (?1, ?2, 1)",
                params![hash, t],
            );
        }
        sync_tags_fts(&conn, &hash);
    }

    get(&hash).ok_or_else(|| "insert did not round-trip".to_string())
}

/// Mirrors a row's tags into the FTS index so `search_media` can match on them.
/// Triggers cannot do this: tags live in another table.
fn sync_tags_fts(conn: &rusqlite::Connection, hash: &str) {
    let tags = tags_for(conn, hash).join(" ");
    let _ = conn.execute(
        "UPDATE media_fts SET tags = ?2 WHERE hash = ?1",
        params![hash, tags],
    );
}

fn tags_for(conn: &rusqlite::Connection, hash: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT tag FROM tag WHERE media_hash = ?1 ORDER BY tag")
    else {
        return Vec::new();
    };
    stmt.query_map([hash], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

fn row_to_item(conn: &rusqlite::Connection, row: &rusqlite::Row) -> rusqlite::Result<MediaItem> {
    let hash: String = row.get("hash")?;
    let ext: String = row.get("ext")?;
    let thumb = store::cache()
        .query_row(
            "SELECT path FROM thumb WHERE hash = ?1 ORDER BY size DESC LIMIT 1",
            [&hash],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .filter(|p| Path::new(p).is_file());

    Ok(MediaItem {
        path: store_path(&hash, &ext)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        tags: tags_for(conn, &hash),
        thumb,
        bytes: row.get::<_, i64>("bytes")? as u64,
        width: row.get("width")?,
        height: row.get("height")?,
        orientation: row.get("orientation")?,
        dominant: row.get("dominant")?,
        origin: row.get("origin")?,
        title: row.get("title")?,
        note: row.get("note")?,
        added_at: row.get("added_at")?,
        kind: row.get("kind")?,
        hash,
        ext,
    })
}

pub fn get(hash: &str) -> Option<MediaItem> {
    let conn = store::data();
    conn.query_row("SELECT * FROM media WHERE hash = ?1", [hash], |r| {
        row_to_item(&conn, r)
    })
    .ok()
}

/// Newest first, optionally scoped to a collection.
pub fn list(collection: Option<i64>) -> Vec<MediaItem> {
    let conn = store::data();
    let sql = match collection {
        Some(_) => {
            "SELECT m.* FROM media m
             JOIN collection_media cm ON cm.media_hash = m.hash
             WHERE cm.collection_id = ?1
             ORDER BY m.added_at DESC"
        }
        None => "SELECT * FROM media ORDER BY added_at DESC",
    };
    let Ok(mut stmt) = conn.prepare(sql) else {
        return Vec::new();
    };
    // Collected inside each arm: `query_map` borrows the params, so the two
    // branches have different types and cannot share a binding.
    match collection {
        Some(id) => stmt
            .query_map(params![id], |r| row_to_item(&conn, r))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default(),
        None => stmt
            .query_map([], |r| row_to_item(&conn, r))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default(),
    }
}

/// Full-text search over title, note, origin and tags.
pub fn search(query: &str, limit: usize) -> Vec<MediaItem> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return list(None).into_iter().take(limit).collect();
    }
    // Prefix-match the final term so search feels live as you type. The whole
    // query is quoted to keep FTS operators in user input from erroring.
    let fts = format!("\"{}\"*", trimmed.replace('"', ""));

    let conn = store::data();
    let Ok(mut stmt) = conn.prepare(
        "SELECT m.* FROM media_fts f
         JOIN media m ON m.hash = f.hash
         WHERE media_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![fts, limit as i64], |r| row_to_item(&conn, r))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn remove(hash: &str) -> Result<(), String> {
    // Cascades clear tags and collection membership.
    store::data()
        .execute("DELETE FROM media WHERE hash = ?1", [hash])
        .map_err(|e| e.to_string())?;
    let _ = store::cache().execute("DELETE FROM thumb WHERE hash = ?1", [hash]);
    if let Some(dir) = store::media_dir().map(|d| d.join(hash)) {
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(())
}

pub fn set_tags(hash: &str, tags: &[String]) -> Result<(), String> {
    let conn = store::data();
    // Only user tags are replaced; derived ones (auto = 1) are kept.
    conn.execute("DELETE FROM tag WHERE media_hash = ?1 AND auto = 0", [hash])
        .map_err(|e| e.to_string())?;
    for t in tags {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        conn.execute(
            "INSERT OR IGNORE INTO tag(media_hash, tag, auto) VALUES (?1, ?2, 0)",
            params![hash, t],
        )
        .map_err(|e| e.to_string())?;
    }
    sync_tags_fts(&conn, hash);
    Ok(())
}

// ---------------------------------------------------------- collections ---

pub fn collections() -> Vec<Collection> {
    let conn = store::data();
    let Ok(mut stmt) = conn.prepare(
        "SELECT c.id, c.name, count(cm.media_hash)
         FROM collection c
         LEFT JOIN collection_media cm ON cm.collection_id = c.id
         GROUP BY c.id ORDER BY c.name",
    ) else {
        return Vec::new();
    };
    stmt.query_map([], |r| {
        Ok(Collection {
            id: r.get(0)?,
            name: r.get(1)?,
            count: r.get(2)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

pub fn create_collection(name: &str) -> Result<i64, String> {
    let conn = store::data();
    conn.execute(
        "INSERT OR IGNORE INTO collection(name, created_at) VALUES (?1, ?2)",
        params![name, store::now()],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row("SELECT id FROM collection WHERE name = ?1", [name], |r| {
        r.get(0)
    })
    .map_err(|e| e.to_string())
}

pub fn set_membership(collection_id: i64, hash: &str, member: bool) -> Result<(), String> {
    let conn = store::data();
    if member {
        conn.execute(
            "INSERT OR IGNORE INTO collection_media(collection_id, media_hash, added_at)
             VALUES (?1, ?2, ?3)",
            params![collection_id, hash, store::now()],
        )
    } else {
        conn.execute(
            "DELETE FROM collection_media WHERE collection_id = ?1 AND media_hash = ?2",
            params![collection_id, hash],
        )
    }
    .map(|_| ())
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Anchored on the Aviary palette itself: if the board cannot name its own
    /// brand colours the way the design system does, the filter chips are
    /// useless.
    #[test]
    fn names_hues_usefully() {
        assert_eq!(colour_name("#3c9b9a"), Some("teal")); // tidal.1
        assert_eq!(colour_name("#75b9f0"), Some("blue")); // aurora.2
        assert_eq!(colour_name("#8d7ae8"), Some("violet")); // aurora.1
        assert_eq!(colour_name("#b34e78"), Some("magenta")); // dusk.3
        assert_eq!(colour_name("#e66b66"), Some("red")); // ember.1
        assert_eq!(colour_name("#fde68a"), Some("gold")); // accent/gold
        assert_eq!(colour_name("#ffd68f"), Some("orange")); // ember.3 is apricot
        assert_eq!(colour_name("#0a0a0b"), Some("dark"));
        assert_eq!(colour_name("#f4f4f6"), Some("light"));
    }

    #[test]
    fn average_colour_is_a_hex_triplet() {
        let img = image::DynamicImage::new_rgb8(8, 8);
        let hex = average_colour(&img);
        assert_eq!(hex.len(), 7);
        assert!(hex.starts_with('#'));
        assert!(u32::from_str_radix(&hex[1..], 16).is_ok());
    }

    /// End-to-end against the real store: import a generated image, then check
    /// dedupe, thumbnailing, auto-tags and search all agree. Removes itself, so
    /// the user's board is left exactly as it was.
    #[test]
    fn imports_dedupes_and_searches() {
        let dir = std::env::temp_dir().join("aviary-media-test");
        let _ = std::fs::create_dir_all(&dir);
        let src = dir.join("aviary-test-teal.png");

        // A flat teal image: the colour is what the auto-tagger should name.
        let mut img = image::RgbImage::new(64, 32);
        for px in img.pixels_mut() {
            *px = image::Rgb([0x3c, 0x9b, 0x9a]);
        }
        image::DynamicImage::ImageRgb8(img).save(&src).unwrap();

        let item = import(&src).expect("import should succeed");
        let hash = item.hash.clone();

        // Cleanup runs even if an assertion below fails.
        let result = std::panic::catch_unwind(|| {
            let item = get(&hash).expect("must be readable back");
            assert_eq!((item.width, item.height), (Some(64), Some(32)));
            assert_eq!(item.orientation.as_deref(), Some("landscape"));
            assert!(item.tags.contains(&"teal".to_string()), "tags: {:?}", item.tags);
            assert!(item.tags.contains(&"landscape".to_string()));
            assert!(
                Path::new(&item.path).is_file(),
                "the original must be copied into the store"
            );
            assert!(
                item.thumb.as_deref().map(|t| Path::new(t).is_file()) == Some(true),
                "a thumbnail must be generated"
            );

            // Re-importing the same bytes must not create a second tile.
            let again = import(&src).expect("second import should succeed");
            assert_eq!(again.hash, hash, "content addressing must dedupe");

            // The MCP server's search path must find it by its derived tag.
            let hits = search("teal", 50);
            assert!(
                hits.iter().any(|h| h.hash == hash),
                "search('teal') must return the imported item"
            );
        });

        remove(&hash).expect("cleanup must succeed");
        let _ = std::fs::remove_file(&src);
        assert!(get(&hash).is_none(), "cleanup must leave no row behind");

        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn classifies_extensions() {
        assert_eq!(kind_of("png"), "image");
        assert_eq!(kind_of("mov"), "video");
        assert_eq!(kind_of("pdf"), "file");
    }
}
