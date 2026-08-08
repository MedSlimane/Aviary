//! Read-only MCP tools for Aviary's content-addressed media board.
//!
//! A server may be restricted to one collection. The scope is carried into
//! every lookup, including direct hash fetches, so knowing an out-of-scope hash
//! never bypasses the collection boundary.

use crate::mcp_protocol::{first_extra_key, ToolError, ToolResponse, ToolServer};
use crate::{media, media::MediaItem};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const MAX_QUERY_CHARS: usize = 512;
const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 50;

pub trait MediaSource: Send + Sync {
    fn collection(&self, id: i64) -> Result<Option<media::Collection>, String>;
    fn collections(&self) -> Result<Vec<media::Collection>, String>;
    fn search(
        &self,
        query: &str,
        limit: usize,
        collection: Option<i64>,
    ) -> Result<Vec<MediaItem>, String>;
    fn get(&self, hash: &str, collection: Option<i64>) -> Result<Option<MediaItem>, String>;
}

pub struct LiveMediaSource {
    data_path: PathBuf,
    cache_path: PathBuf,
    media_root: PathBuf,
}

impl LiveMediaSource {
    fn current() -> Result<Self, String> {
        let root = crate::store::dir().ok_or("no home directory")?;
        let data_path = root.join("data.db");
        if !data_path.is_file() {
            return Err("data.db is unavailable; open Aviary to create it".into());
        }
        Ok(Self {
            cache_path: root.join("cache.db"),
            media_root: root.join("media"),
            data_path,
        })
    }

    fn data(&self) -> Result<Connection, String> {
        open_read_only(&self.data_path)
    }

    fn row_to_item(&self, connection: &Connection, row: &Row<'_>) -> rusqlite::Result<MediaItem> {
        let hash: String = row.get("hash")?;
        let ext: String = row.get("ext")?;
        let tags = {
            let mut statement =
                connection.prepare("SELECT tag FROM tag WHERE media_hash = ?1 ORDER BY tag")?;
            let tags = statement
                .query_map([&hash], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            tags
        };
        let thumb = open_read_only(&self.cache_path)
            .ok()
            .and_then(|cache| {
                cache
                    .query_row(
                        "SELECT path FROM thumb WHERE hash = ?1 ORDER BY size DESC LIMIT 1",
                        [&hash],
                        |row| row.get::<_, String>(0),
                    )
                    .ok()
            })
            .filter(|path| Path::new(path).is_file());
        Ok(MediaItem {
            path: self
                .media_root
                .join(&hash)
                .join(format!("original.{ext}"))
                .to_string_lossy()
                .into_owned(),
            tags,
            thumb,
            bytes: row.get::<_, u64>("bytes")?,
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
}

fn open_read_only(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    connection
        .pragma_update(None, "query_only", "ON")
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

impl MediaSource for LiveMediaSource {
    fn collection(&self, id: i64) -> Result<Option<media::Collection>, String> {
        let connection = self.data()?;
        connection
            .query_row(
                "SELECT c.id, c.name, count(cm.media_hash)
                   FROM collection c
                   LEFT JOIN collection_media cm ON cm.collection_id = c.id
                  WHERE c.id = ?1 GROUP BY c.id",
                [id],
                |row| {
                    Ok(media::Collection {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        count: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn collections(&self) -> Result<Vec<media::Collection>, String> {
        let connection = self.data()?;
        let mut statement = connection
            .prepare(
                "SELECT c.id, c.name, count(cm.media_hash)
                   FROM collection c
                   LEFT JOIN collection_media cm ON cm.collection_id = c.id
                  GROUP BY c.id ORDER BY c.name, c.id",
            )
            .map_err(|error| error.to_string())?;
        let collections = statement
            .query_map([], |row| {
                Ok(media::Collection {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    count: row.get(2)?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
        Ok(collections)
    }

    fn search(
        &self,
        query: &str,
        limit: usize,
        collection: Option<i64>,
    ) -> Result<Vec<MediaItem>, String> {
        let connection = self.data()?;
        let trimmed = query.trim();
        let (sql, fts) = if trimmed.is_empty() {
            (
                match collection {
                    Some(_) => {
                        "SELECT m.* FROM media m
                         JOIN collection_media cm ON cm.media_hash = m.hash
                         WHERE cm.collection_id = ?1
                         ORDER BY m.added_at DESC, m.hash LIMIT ?2"
                    }
                    None => "SELECT m.* FROM media m ORDER BY m.added_at DESC, m.hash LIMIT ?1",
                },
                None,
            )
        } else {
            (
                match collection {
                    Some(_) => {
                        "SELECT m.* FROM media_fts f
                         JOIN media m ON m.hash = f.hash
                         JOIN collection_media cm ON cm.media_hash = m.hash
                         WHERE media_fts MATCH ?1 AND cm.collection_id = ?2
                         ORDER BY rank, m.added_at DESC, m.hash LIMIT ?3"
                    }
                    None => {
                        "SELECT m.* FROM media_fts f
                         JOIN media m ON m.hash = f.hash
                         WHERE media_fts MATCH ?1
                         ORDER BY rank, m.added_at DESC, m.hash LIMIT ?2"
                    }
                },
                Some(format!("\"{}\"*", trimmed.replace('"', ""))),
            )
        };
        let mut statement = connection.prepare(sql).map_err(|error| error.to_string())?;
        let map = |row: &Row<'_>| self.row_to_item(&connection, row);
        let items = match (fts.as_deref(), collection) {
            (None, Some(collection)) => statement.query_map(params![collection, limit], map),
            (None, None) => statement.query_map(params![limit], map),
            (Some(fts), Some(collection)) => {
                statement.query_map(params![fts, collection, limit], map)
            }
            (Some(fts), None) => statement.query_map(params![fts, limit], map),
        }
        .map_err(|error| error.to_string())?;
        items
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())
    }

    fn get(&self, hash: &str, collection: Option<i64>) -> Result<Option<MediaItem>, String> {
        let connection = self.data()?;
        let result = match collection {
            Some(id) => connection.query_row(
                "SELECT m.* FROM media m
                 JOIN collection_media cm ON cm.media_hash = m.hash
                 WHERE m.hash = ?1 AND cm.collection_id = ?2",
                params![hash, id],
                |row| self.row_to_item(&connection, row),
            ),
            None => {
                connection.query_row("SELECT m.* FROM media m WHERE m.hash = ?1", [hash], |row| {
                    self.row_to_item(&connection, row)
                })
            }
        };
        result.optional().map_err(|error| error.to_string())
    }
}

pub struct MediaServer<S = LiveMediaSource> {
    source: S,
    collection: Option<i64>,
}

impl MediaServer<LiveMediaSource> {
    pub fn current(collection: Option<i64>) -> Result<Self, String> {
        Self::from_source(LiveMediaSource::current()?, collection)
    }
}

impl<S: MediaSource> MediaServer<S> {
    pub fn from_source(source: S, collection: Option<i64>) -> Result<Self, String> {
        if let Some(id) = collection {
            if id <= 0 {
                return Err("collection id must be positive".into());
            }
            if source.collection(id)?.is_none() {
                return Err(format!("media collection {id} does not exist"));
            }
        }
        Ok(Self { source, collection })
    }

    fn search_media(&self, arguments: &Map<String, Value>) -> Result<ToolResponse, ToolError> {
        reject_extra(arguments, &["query", "limit"])?;
        let query = required_string(arguments, "query", MAX_QUERY_CHARS)?;
        let limit = bounded_limit(arguments)?;
        let items = self
            .source
            .search(&query, limit, self.collection)
            .map_err(ToolError::Failed)?;
        let structured = json!({
            "items": items,
            "returned": items.len(),
            "collectionId": self.collection
        });
        let text = if items.is_empty() {
            format!("No media matched {query:?} in the available scope.")
        } else {
            format!(
                "{} media item(s):\n\n{}",
                items.len(),
                items.iter().map(describe).collect::<Vec<_>>().join("\n\n")
            )
        };
        Ok(ToolResponse::new(structured, text))
    }

    fn get_media(&self, arguments: &Map<String, Value>) -> Result<ToolResponse, ToolError> {
        reject_extra(arguments, &["hash"])?;
        let hash = required_string(arguments, "hash", 128)?;
        if !hash
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(ToolError::InvalidArguments(
                "hash contains unsupported characters".into(),
            ));
        }
        let Some(item) = self
            .source
            .get(&hash, self.collection)
            .map_err(ToolError::Failed)?
        else {
            return Err(ToolError::Failed(
                "No media with that hash exists in the available scope.".into(),
            ));
        };
        let text = describe(&item);
        Ok(ToolResponse::new(
            json!({ "item": item, "collectionId": self.collection }),
            text,
        ))
    }

    fn list_collections(&self, arguments: &Map<String, Value>) -> Result<ToolResponse, ToolError> {
        reject_extra(arguments, &[])?;
        let collections = self
            .source
            .collections()
            .map_err(ToolError::Failed)?
            .into_iter()
            .filter(|collection| self.collection.is_none_or(|id| collection.id == id))
            .collect::<Vec<_>>();
        let text = if collections.is_empty() {
            "No collections are available in this scope.".into()
        } else {
            collections
                .iter()
                .map(|collection| format!("{} ({} items)", collection.name, collection.count))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(ToolResponse::new(
            json!({ "collections": collections, "returned": collections.len() }),
            text,
        ))
    }
}

impl<S: MediaSource> ToolServer for MediaServer<S> {
    fn name(&self) -> &'static str {
        "aviary-media"
    }

    fn instructions(&self) -> Option<&'static str> {
        Some("Search and fetch real items from Aviary's read-only media board.")
    }

    fn tools(&self) -> Vec<Value> {
        vec![
            json!({
                "name": "search_media",
                "description": "Search media titles, notes, original filenames and tags in the configured board scope.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "minLength": 1, "maxLength": MAX_QUERY_CHARS },
                        "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "items": { "type": "array", "maxItems": MAX_LIMIT, "items": media_item_schema() },
                        "returned": { "type": "integer", "minimum": 0, "maximum": MAX_LIMIT },
                        "collectionId": { "type": ["integer", "null"] }
                    },
                    "required": ["items", "returned", "collectionId"],
                    "additionalProperties": false
                },
                "annotations": read_only_annotations()
            }),
            json!({
                "name": "get_media",
                "description": "Fetch one media item by content hash within the configured board scope.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "hash": { "type": "string", "minLength": 1, "maxLength": 128 } },
                    "required": ["hash"],
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "item": media_item_schema(),
                        "collectionId": { "type": ["integer", "null"] }
                    },
                    "required": ["item", "collectionId"],
                    "additionalProperties": false
                },
                "annotations": read_only_annotations()
            }),
            json!({
                "name": "list_collections",
                "description": "List media collections visible to this configured server scope.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "collections": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "integer" },
                                    "name": { "type": "string" },
                                    "count": { "type": "integer", "minimum": 0 }
                                },
                                "required": ["id", "name", "count"],
                                "additionalProperties": false
                            }
                        },
                        "returned": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["collections", "returned"],
                    "additionalProperties": false
                },
                "annotations": read_only_annotations()
            }),
        ]
    }

    fn call(&self, name: &str, arguments: &Map<String, Value>) -> Result<ToolResponse, ToolError> {
        match name {
            "search_media" => self.search_media(arguments),
            "get_media" => self.get_media(arguments),
            "list_collections" => self.list_collections(arguments),
            other => Err(ToolError::UnknownTool(other.into())),
        }
    }
}

fn media_item_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "hash": { "type": "string" },
            "kind": { "type": "string" },
            "ext": { "type": "string" },
            "bytes": { "type": "integer", "minimum": 0 },
            "width": { "type": ["integer", "null"] },
            "height": { "type": ["integer", "null"] },
            "orientation": { "type": ["string", "null"] },
            "dominant": { "type": ["string", "null"] },
            "origin": { "type": ["string", "null"] },
            "title": { "type": ["string", "null"] },
            "note": { "type": ["string", "null"] },
            "added_at": { "type": "integer" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "path": { "type": "string" },
            "thumb": { "type": ["string", "null"] }
        },
        "required": [
            "hash", "kind", "ext", "bytes", "width", "height", "orientation",
            "dominant", "origin", "title", "note", "added_at", "tags", "path", "thumb"
        ],
        "additionalProperties": false
    })
}

fn read_only_annotations() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    })
}

fn reject_extra(arguments: &Map<String, Value>, allowed: &[&str]) -> Result<(), ToolError> {
    if let Some(extra) = first_extra_key(arguments, allowed) {
        return Err(ToolError::InvalidArguments(format!(
            "unknown argument: {extra}"
        )));
    }
    Ok(())
}

fn required_string(
    arguments: &Map<String, Value>,
    field: &str,
    max_chars: usize,
) -> Result<String, ToolError> {
    let value = arguments
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments(format!("{field} is required")))?;
    let count = value.chars().count();
    if count == 0 || count > max_chars {
        return Err(ToolError::InvalidArguments(format!(
            "{field} must contain 1 to {max_chars} characters"
        )));
    }
    Ok(value.to_string())
}

fn bounded_limit(arguments: &Map<String, Value>) -> Result<usize, ToolError> {
    match arguments.get("limit") {
        None => Ok(DEFAULT_LIMIT),
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|limit| (1..=MAX_LIMIT as u64).contains(limit))
            .map(|limit| limit as usize)
            .ok_or_else(|| {
                ToolError::InvalidArguments(format!("limit must be between 1 and {MAX_LIMIT}"))
            }),
        Some(_) => Err(ToolError::InvalidArguments(
            "limit must be an integer".into(),
        )),
    }
}

fn describe(item: &MediaItem) -> String {
    let mut lines = vec![format!("path: {}", item.path)];
    if let Some(title) = &item.title {
        lines.push(format!("title: {title}"));
    }
    if let (Some(width), Some(height)) = (item.width, item.height) {
        lines.push(format!("size: {width}x{height}"));
    }
    if let Some(colour) = &item.dominant {
        lines.push(format!("colour: {colour}"));
    }
    if !item.tags.is_empty() {
        lines.push(format!("tags: {}", item.tags.join(", ")));
    }
    lines.push(format!("hash: {}", item.hash));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_protocol::{Runtime, PROTOCOL_VERSION};

    struct FixtureSource;

    impl FixtureSource {
        fn item(hash: &str) -> MediaItem {
            MediaItem {
                hash: hash.into(),
                kind: "image".into(),
                ext: "png".into(),
                bytes: 10,
                width: Some(2),
                height: Some(1),
                orientation: Some("landscape".into()),
                dominant: Some("#001122".into()),
                origin: None,
                title: Some("Fixture".into()),
                note: None,
                added_at: 1,
                tags: vec!["teal".into()],
                path: format!("/media/{hash}.png"),
                thumb: None,
            }
        }
    }

    impl MediaSource for FixtureSource {
        fn collection(&self, id: i64) -> Result<Option<media::Collection>, String> {
            Ok((id == 7).then(|| media::Collection {
                id,
                name: "Scoped".into(),
                count: 1,
            }))
        }

        fn collections(&self) -> Result<Vec<media::Collection>, String> {
            Ok(vec![
                media::Collection {
                    id: 7,
                    name: "Scoped".into(),
                    count: 1,
                },
                media::Collection {
                    id: 8,
                    name: "Private".into(),
                    count: 1,
                },
            ])
        }

        fn search(
            &self,
            _query: &str,
            _limit: usize,
            collection: Option<i64>,
        ) -> Result<Vec<MediaItem>, String> {
            Ok(vec![Self::item(if collection == Some(7) {
                "scoped"
            } else {
                "unscoped"
            })])
        }

        fn get(&self, hash: &str, collection: Option<i64>) -> Result<Option<MediaItem>, String> {
            Ok(match (hash, collection) {
                ("scoped", Some(7)) => Some(Self::item(hash)),
                ("outside", None) => Some(Self::item(hash)),
                _ => None,
            })
        }
    }

    fn ready_runtime() -> Runtime<MediaServer<FixtureSource>> {
        let mut runtime = Runtime::new(MediaServer::from_source(FixtureSource, Some(7)).unwrap());
        runtime.handle_line(
            &json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": PROTOCOL_VERSION }
            })
            .to_string(),
        );
        runtime.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        runtime
    }

    fn call(runtime: &mut Runtime<MediaServer<FixtureSource>>, name: &str, args: Value) -> Value {
        let response = runtime
            .handle_line(
                &json!({
                    "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": name, "arguments": args }
                })
                .to_string(),
            )
            .unwrap();
        serde_json::from_str(&response).unwrap()
    }

    #[test]
    fn collection_scope_reaches_search_get_and_collection_listing() {
        let mut runtime = ready_runtime();
        let search = call(&mut runtime, "search_media", json!({ "query": "teal" }));
        assert_eq!(
            search["result"]["structuredContent"]["items"][0]["hash"],
            "scoped"
        );
        assert_eq!(search["result"]["structuredContent"]["collectionId"], 7);

        let outside = call(&mut runtime, "get_media", json!({ "hash": "outside" }));
        assert_eq!(outside["result"]["isError"], true);
        let scoped = call(&mut runtime, "get_media", json!({ "hash": "scoped" }));
        assert_eq!(scoped["result"]["isError"], false);

        let collections = call(&mut runtime, "list_collections", json!({}));
        let listed = collections["result"]["structuredContent"]["collections"]
            .as_array()
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], 7);
    }

    #[test]
    fn schemas_and_arguments_are_strict_and_read_only() {
        let server = MediaServer::from_source(FixtureSource, Some(7)).unwrap();
        for tool in server.tools() {
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert!(tool["outputSchema"].is_object());
            assert_eq!(tool["annotations"]["readOnlyHint"], true);
            assert_eq!(tool["annotations"]["destructiveHint"], false);
        }
        let mut runtime = ready_runtime();
        let extra = call(
            &mut runtime,
            "search_media",
            json!({ "query": "teal", "path": "/tmp" }),
        );
        assert_eq!(extra["result"]["isError"], true);
    }

    #[test]
    fn missing_collection_refuses_to_start() {
        assert!(MediaServer::from_source(FixtureSource, Some(99)).is_err());
    }

    #[test]
    fn live_source_opens_durable_and_cache_databases_read_only() {
        let root = tempfile::tempdir().unwrap();
        let data_path = root.path().join("data.db");
        let cache_path = root.path().join("cache.db");
        let data = Connection::open(&data_path).unwrap();
        data.execute_batch(
            "CREATE TABLE media(
                hash TEXT PRIMARY KEY, kind TEXT, ext TEXT, bytes INTEGER,
                width INTEGER, height INTEGER, orientation TEXT, dominant TEXT,
                origin TEXT, title TEXT, note TEXT, added_at INTEGER
             );
             CREATE TABLE tag(media_hash TEXT, tag TEXT);
             CREATE TABLE collection(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE collection_media(collection_id INTEGER, media_hash TEXT);
             INSERT INTO media(hash, kind, ext, bytes, title, added_at)
             VALUES ('abc', 'image', 'png', 10, 'Real', 1);
             INSERT INTO tag(media_hash, tag) VALUES ('abc', 'teal');",
        )
        .unwrap();
        drop(data);
        let cache = Connection::open(&cache_path).unwrap();
        cache
            .execute_batch("CREATE TABLE thumb(hash TEXT, size INTEGER, path TEXT);")
            .unwrap();
        drop(cache);
        let source = LiveMediaSource {
            data_path: data_path.clone(),
            cache_path,
            media_root: root.path().join("media"),
        };
        let item = source.get("abc", None).unwrap().unwrap();
        assert_eq!(item.tags, vec!["teal".to_string()]);
        let writable = Connection::open(&data_path).unwrap();
        writable
            .execute("UPDATE media SET bytes = -1 WHERE hash = 'abc'", [])
            .unwrap();
        drop(writable);
        assert!(source.get("abc", None).is_err());
        let read_only = open_read_only(&data_path).unwrap();
        assert!(read_only
            .execute("UPDATE media SET title = 'Changed' WHERE hash = 'abc'", [])
            .is_err());
    }
}
