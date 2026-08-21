//! Auracle's independently usable, streaming JSONL v1 exporter.

use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    io::{BufWriter, Cursor, Write},
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, SecondsFormat, Utc};
use imessage_database::{
    message_types::{
        edited::{EditStatus, EditedMessage},
        variants::BalloonProvider,
    },
    util::streamtyped,
};
use plist::Value as PlistValue;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: u8 = 1;
/// How many messages a pass streams between `progress` records. Auracle
/// batches 500 records, so a resume point lands inside nearly every batch.
pub const DEFAULT_PROGRESS_EVERY: u64 = 200;
const APPLE_EPOCH_UNIX_SECONDS: i64 = 978_307_200;
const NANOSECOND_THRESHOLD: i64 = 1_000_000_000_000;

#[derive(Debug)]
pub enum ExportError {
    DatabaseOpen,
    DatabaseSchema,
    DatabaseRead,
    InvalidCursor,
    InvalidTimestamp,
    Output,
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DatabaseOpen => "database_open",
            Self::DatabaseSchema => "database_schema",
            Self::DatabaseRead => "database_read",
            Self::InvalidCursor => "invalid_cursor",
            Self::InvalidTimestamp => "invalid_timestamp",
            Self::Output => "output_write",
        })
    }
}

impl std::error::Error for ExportError {}

#[derive(Debug)]
pub struct ExportOptions {
    pub db_path: PathBuf,
    pub cursor: Option<String>,
    /// The cursor from a `progress` record of an interrupted pass. Messages
    /// that pass had already streamed are skipped; handles and chats are
    /// always re-emitted so the resumed stream stands on its own.
    pub resume: Option<String>,
    /// Emit a `progress` record after this many messages; zero emits none.
    pub progress_every: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct CursorV1 {
    version: u8,
    database_fingerprint: String,
    last_message_rowid: i64,
    last_edited_at: i64,
    #[serde(default)]
    last_message_date: i64,
}

#[derive(Serialize)]
struct Manifest<'a> {
    #[serde(rename = "type")]
    record_type: &'static str,
    schema_version: u8,
    tool_version: &'a str,
    database_fingerprint: &'a str,
    export_mode: &'static str,
}

#[derive(Serialize)]
struct Handle {
    #[serde(rename = "type")]
    record_type: &'static str,
    source_id: String,
    service: String,
    value: String,
}

#[derive(Serialize)]
struct Chat {
    #[serde(rename = "type")]
    record_type: &'static str,
    guid: String,
    service: String,
    kind: &'static str,
    participants: Vec<String>,
}

#[derive(Serialize)]
struct Message {
    #[serde(rename = "type")]
    record_type: &'static str,
    guid: String,
    chat_guid: String,
    sender: Option<String>,
    direction: &'static str,
    sent_at: String,
    edited_at: Option<String>,
    text: Option<String>,
    edit_state: &'static str,
    reactions: Vec<Reaction>,
}

#[derive(Clone, Serialize)]
struct Reaction {
    source_id: String,
    sender: Option<String>,
    kind: String,
    removed: bool,
}

#[derive(Serialize)]
struct Attachment {
    #[serde(rename = "type")]
    record_type: &'static str,
    source_id: String,
    message_guid: String,
    mime_type: Option<String>,
    basename: Option<String>,
    size_bytes: Option<i64>,
    available: bool,
}

/// A resumable position inside one pass. Its cursor is a `CursorV1` whose
/// `last_message_rowid` is the message just streamed rather than the
/// database's last row, so `--resume` can pick the pass up from there.
#[derive(Serialize)]
struct Progress {
    #[serde(rename = "type")]
    record_type: &'static str,
    cursor: String,
}

#[derive(Serialize)]
struct Checkpoint {
    #[serde(rename = "type")]
    record_type: &'static str,
    cursor: String,
    totals: Totals,
}

#[derive(Serialize)]
struct Totals {
    records: u64,
    messages: u64,
    chats: u64,
}

struct Emitter<W: Write> {
    output: BufWriter<W>,
    records: u64,
}

impl<W: Write> Emitter<W> {
    fn new(output: W) -> Self {
        Self {
            output: BufWriter::new(output),
            records: 0,
        }
    }

    fn record<T: Serialize>(&mut self, record: &T) -> Result<(), ExportError> {
        serde_json::to_writer(&mut self.output, record).map_err(|_| ExportError::Output)?;
        self.output
            .write_all(b"\n")
            .map_err(|_| ExportError::Output)?;
        self.records += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<(), ExportError> {
        self.output.flush().map_err(|_| ExportError::Output)
    }
}

/// A span of the archive an earlier pass already carried, read from a cursor.
///
/// A message is inside the window when nothing about it has changed since
/// that pass: created no later than the cursor's last row, edited no later
/// than its edit high-water mark, and not the target of a reaction that
/// arrived after its last message date. An incremental pass has one window
/// (the previous checkpoint); a resumed pass adds a second (the progress
/// point), and a message is streamed only when it is outside every window.
struct Window {
    last_message_rowid: i64,
    last_edited_at: i64,
    reaction_targets: HashSet<String>,
}

impl Window {
    fn open(
        database: &Connection,
        message_columns: &HashSet<String>,
        cursor: &CursorV1,
    ) -> Result<Self, ExportError> {
        Ok(Self {
            last_message_rowid: cursor.last_message_rowid,
            last_edited_at: cursor.last_edited_at,
            reaction_targets: changed_reaction_targets(
                database,
                message_columns,
                cursor.last_message_rowid,
                cursor.last_message_date,
            )?,
        })
    }

    fn contains(&self, rowid: i64, edited_at: i64, guid: &str) -> bool {
        rowid <= self.last_message_rowid
            && edited_at <= self.last_edited_at
            && !self.reaction_targets.contains(guid)
    }
}

/// The snapshot's high-water marks, read once. The deferred read transaction
/// pins one snapshot for the whole pass, so the checkpoint and every progress
/// record in between describe the same instant.
struct SnapshotMarks {
    last_message_rowid: i64,
    last_edited_at: i64,
    last_message_date: i64,
}

impl SnapshotMarks {
    fn read(database: &Connection, message_columns: &HashSet<String>) -> Result<Self, ExportError> {
        let last_message_rowid: i64 = database
            .query_row("SELECT COALESCE(MAX(ROWID), 0) FROM message", [], |row| {
                row.get(0)
            })
            .map_err(|_| ExportError::DatabaseRead)?;
        let last_edited_at = if message_columns.contains("date_edited") {
            database
                .query_row(
                    "SELECT COALESCE(MAX(date_edited), 0) FROM message",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| ExportError::DatabaseRead)?
        } else {
            0
        };
        let last_message_date: i64 = database
            .query_row("SELECT COALESCE(MAX(date), 0) FROM message", [], |row| {
                row.get(0)
            })
            .map_err(|_| ExportError::DatabaseRead)?;
        Ok(Self {
            last_message_rowid,
            last_edited_at,
            last_message_date,
        })
    }

    fn cursor(&self, fingerprint: &str, last_message_rowid: i64) -> CursorV1 {
        CursorV1 {
            version: SCHEMA_VERSION,
            database_fingerprint: fingerprint.to_owned(),
            last_message_rowid,
            last_edited_at: self.last_edited_at,
            last_message_date: self.last_message_date,
        }
    }
}

/// Everything the message loop needs beyond the database itself.
struct MessageScan<'a> {
    windows: Vec<Window>,
    reaction_index: HashMap<String, Vec<Reaction>>,
    fingerprint: &'a str,
    marks: &'a SnapshotMarks,
    progress_every: u64,
}

/// Export one consistent, read-only SQLite snapshot to newline-delimited JSON.
///
/// Message and attachment rows are serialized directly to `output` as SQLite
/// yields them. A compact reaction index (identifiers and type metadata, never
/// canonical message text or attachment data) is built once to avoid an
/// O(messages x reactions) database scan.
pub fn export_jsonl<W: Write>(options: &ExportOptions, output: W) -> Result<(), ExportError> {
    let database = Connection::open_with_flags(
        &options.db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ExportError::DatabaseOpen)?;
    database
        .execute_batch("PRAGMA query_only = ON; BEGIN DEFERRED;")
        .map_err(|_| ExportError::DatabaseRead)?;

    require_tables(&database)?;
    let message_columns = columns(&database, "message")?;
    let chat_columns = columns(&database, "chat")?;
    let handle_columns = columns(&database, "handle")?;
    let attachment_columns = columns(&database, "attachment")?;
    let fingerprint = database_fingerprint(&database, &chat_columns)?;
    let prior = options.cursor.as_deref().map(decode_cursor).transpose()?;
    let incremental = prior
        .as_ref()
        .is_some_and(|cursor| cursor.database_fingerprint == fingerprint);
    let resume = match options.resume.as_deref().map(decode_cursor).transpose()? {
        Some(cursor) if cursor.database_fingerprint == fingerprint => Some(cursor),
        Some(_) => {
            // A progress point inside another database is no position at all.
            // Say so as a category and stream the whole pass; the segment the
            // caller opens is valid either way, only longer.
            eprintln!("diagnostic=resume_ignored reason=database_fingerprint");
            None
        }
        None => None,
    };
    let marks = SnapshotMarks::read(&database, &message_columns)?;

    let mut emitter = Emitter::new(output);
    emitter.record(&Manifest {
        record_type: "manifest",
        schema_version: SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION"),
        database_fingerprint: &fingerprint,
        export_mode: if incremental { "incremental" } else { "full" },
    })?;

    emit_handles(&database, &handle_columns, &mut emitter)?;
    let chat_count = emit_chats(&database, &chat_columns, &mut emitter)?;

    let mut windows = Vec::new();
    if let Some(prior) = prior.filter(|_| incremental) {
        windows.push(Window::open(&database, &message_columns, &prior)?);
    }
    if let Some(resume) = resume {
        windows.push(Window::open(&database, &message_columns, &resume)?);
    }
    let scan = MessageScan {
        windows,
        reaction_index: load_reactions(&database, &message_columns)?,
        fingerprint: &fingerprint,
        marks: &marks,
        progress_every: options.progress_every,
    };
    let message_count = emit_messages(
        &database,
        &message_columns,
        &chat_columns,
        &attachment_columns,
        &scan,
        &mut emitter,
    )?;
    report_unassociated_messages(&database, &message_columns)?;

    let next_cursor = encode_cursor(&marks.cursor(&fingerprint, marks.last_message_rowid))?;
    let final_records = emitter.records + 1;
    emitter.record(&Checkpoint {
        record_type: "checkpoint",
        cursor: next_cursor,
        totals: Totals {
            records: final_records,
            messages: message_count,
            chats: chat_count,
        },
    })?;
    emitter.finish()
}

fn require_tables(database: &Connection) -> Result<(), ExportError> {
    for table in [
        "message",
        "chat",
        "handle",
        "attachment",
        "chat_message_join",
        "chat_handle_join",
        "message_attachment_join",
    ] {
        if !table_exists(database, table)? {
            return Err(ExportError::DatabaseSchema);
        }
    }
    Ok(())
}

fn table_exists(database: &Connection, table: &str) -> Result<bool, ExportError> {
    database
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(|_| ExportError::DatabaseRead)
}

fn columns(database: &Connection, table: &str) -> Result<HashSet<String>, ExportError> {
    let mut statement = database
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|_| ExportError::DatabaseSchema)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| ExportError::DatabaseSchema)?;
    rows.collect::<Result<HashSet<_>, _>>()
        .map_err(|_| ExportError::DatabaseSchema)
}

fn database_fingerprint(
    database: &Connection,
    chat_columns: &HashSet<String>,
) -> Result<String, ExportError> {
    let mut digest = Sha256::new();
    digest.update(b"auracle-imessage-database-v1\0");
    for table in ["message", "chat"] {
        let identity = if table == "chat" && !chat_columns.contains("guid") {
            "chat_identifier"
        } else {
            "guid"
        };
        let sql = format!(
            "SELECT {identity} FROM {table} WHERE {identity} IS NOT NULL AND {identity} != '' ORDER BY ROWID LIMIT 1"
        );
        let mut statement = database
            .prepare(&sql)
            .map_err(|_| ExportError::DatabaseSchema)?;
        let values = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| ExportError::DatabaseRead)?;
        for value in values {
            digest.update(value.map_err(|_| ExportError::DatabaseRead)?.as_bytes());
            digest.update([0]);
        }
    }
    let value = digest.finalize();
    let mut fingerprint = String::with_capacity(71);
    fingerprint.push_str("sha256:");
    for byte in value {
        use fmt::Write as _;
        write!(&mut fingerprint, "{byte:02x}").map_err(|_| ExportError::Output)?;
    }
    Ok(fingerprint)
}

fn emit_handles<W: Write>(
    database: &Connection,
    table_columns: &HashSet<String>,
    emitter: &mut Emitter<W>,
) -> Result<(), ExportError> {
    if !table_columns.contains("id") {
        return Err(ExportError::DatabaseSchema);
    }
    let service = optional_text(table_columns, "service", "unknown");
    let sql = format!(
        "SELECT ROWID, id, {service} AS service FROM handle WHERE id IS NOT NULL AND id != '' ORDER BY ROWID"
    );
    let mut statement = database
        .prepare(&sql)
        .map_err(|_| ExportError::DatabaseSchema)?;
    let mut rows = statement.query([]).map_err(|_| ExportError::DatabaseRead)?;
    while let Some(row) = rows.next().map_err(|_| ExportError::DatabaseRead)? {
        let rowid: i64 = row.get(0).map_err(|_| ExportError::DatabaseRead)?;
        emitter.record(&Handle {
            record_type: "handle",
            source_id: handle_source_id(rowid),
            service: short(
                row.get::<_, String>(2)
                    .map_err(|_| ExportError::DatabaseRead)?,
            ),
            value: short(
                row.get::<_, String>(1)
                    .map_err(|_| ExportError::DatabaseRead)?,
            ),
        })?;
    }
    Ok(())
}

fn emit_chats<W: Write>(
    database: &Connection,
    table_columns: &HashSet<String>,
    emitter: &mut Emitter<W>,
) -> Result<u64, ExportError> {
    let identity = chat_identity_expression(table_columns)?;
    let service = optional_text(table_columns, "service_name", "unknown");
    let sql = format!(
        "SELECT c.ROWID, {identity} AS guid, {service} AS service FROM chat c ORDER BY c.ROWID"
    );
    let mut statement = database
        .prepare(&sql)
        .map_err(|_| ExportError::DatabaseSchema)?;
    let mut rows = statement.query([]).map_err(|_| ExportError::DatabaseRead)?;
    let mut count = 0;
    while let Some(row) = rows.next().map_err(|_| ExportError::DatabaseRead)? {
        let chat_rowid: i64 = row.get(0).map_err(|_| ExportError::DatabaseRead)?;
        let mut participant_statement = database
            .prepare_cached(
                "SELECT DISTINCT handle_id FROM chat_handle_join \
                 WHERE chat_id = ?1 ORDER BY handle_id LIMIT 1000",
            )
            .map_err(|_| ExportError::DatabaseSchema)?;
        let participants = participant_statement
            .query_map([chat_rowid], |participant| participant.get::<_, i64>(0))
            .map_err(|_| ExportError::DatabaseRead)?
            .map(|value| value.map(handle_source_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ExportError::DatabaseRead)?;
        let kind = if participants.len() > 1 {
            "group"
        } else {
            "direct"
        };
        emitter.record(&Chat {
            record_type: "chat",
            guid: short(
                row.get::<_, String>(1)
                    .map_err(|_| ExportError::DatabaseRead)?,
            ),
            service: short(
                row.get::<_, String>(2)
                    .map_err(|_| ExportError::DatabaseRead)?,
            ),
            kind,
            participants,
        })?;
        count += 1;
    }
    Ok(count)
}

fn emit_messages<W: Write>(
    database: &Connection,
    message_columns: &HashSet<String>,
    chat_columns: &HashSet<String>,
    attachment_columns: &HashSet<String>,
    scan: &MessageScan<'_>,
    emitter: &mut Emitter<W>,
) -> Result<u64, ExportError> {
    for required in ["guid", "date", "is_from_me"] {
        if !message_columns.contains(required) {
            return Err(ExportError::DatabaseSchema);
        }
    }
    let handle_id = optional_integer(message_columns, "handle_id", "NULL");
    let text = optional_nullable(message_columns, "text");
    let attributed_body = optional_nullable(message_columns, "attributedBody");
    let summary_info = optional_nullable(message_columns, "message_summary_info");
    let date_edited = optional_integer(message_columns, "date_edited", "0");
    let item_type = optional_integer(message_columns, "item_type", "0");
    let group_action = optional_integer(message_columns, "group_action_type", "0");
    let associated_type = optional_integer(message_columns, "associated_message_type", "NULL");
    let chat_identity = chat_identity_expression(chat_columns)?;
    let chat_join = if table_exists(database, "chat_recoverable_message_join")? {
        "SELECT message_id, MIN(chat_id) AS chat_id FROM chat_message_join GROUP BY message_id \
         UNION ALL \
         SELECT r.message_id, MIN(r.chat_id) AS chat_id \
         FROM chat_recoverable_message_join r \
         WHERE NOT EXISTS (SELECT 1 FROM chat_message_join a \
                           WHERE a.message_id = r.message_id) \
         GROUP BY r.message_id"
    } else {
        "SELECT message_id, MIN(chat_id) AS chat_id \
         FROM chat_message_join GROUP BY message_id"
    };
    let sql = format!(
        "SELECT m.ROWID, m.guid, cmj.chat_id, {chat_identity} AS chat_guid, \
         {handle_id} AS handle_id, m.date, m.is_from_me, {text} AS text, \
         {attributed_body} AS attributed_body, {date_edited} AS date_edited, \
         {summary_info} AS message_summary_info, \
         {item_type} AS item_type, {group_action} AS group_action_type, \
         {associated_type} AS associated_message_type \
         FROM message m \
         JOIN ({chat_join}) cmj ON cmj.message_id = m.ROWID \
         JOIN chat c ON c.ROWID = cmj.chat_id \
         WHERE m.guid IS NOT NULL AND m.guid != '' \
         AND m.ROWID = (SELECT MAX(m2.ROWID) FROM message m2 WHERE m2.guid = m.guid) \
         ORDER BY m.ROWID"
    );
    let mut statement = database
        .prepare(&sql)
        .map_err(|_| ExportError::DatabaseSchema)?;
    let mut rows = statement.query([]).map_err(|_| ExportError::DatabaseRead)?;
    let mut count: u64 = 0;
    while let Some(row) = rows.next().map_err(|_| ExportError::DatabaseRead)? {
        let rowid: i64 = row.get(0).map_err(|_| ExportError::DatabaseRead)?;
        let guid: String = row.get(1).map_err(|_| ExportError::DatabaseRead)?;
        let edited_raw: i64 = row.get(9).map_err(|_| ExportError::DatabaseRead)?;
        let item_type: i64 = row.get(11).map_err(|_| ExportError::DatabaseRead)?;
        let group_action: i64 = row.get(12).map_err(|_| ExportError::DatabaseRead)?;
        let associated_type: Option<i64> = row.get(13).map_err(|_| ExportError::DatabaseRead)?;
        let is_associated_event = !is_canonical_association(associated_type);
        let is_service = item_type != 0 || group_action != 0;
        if is_associated_event || is_service {
            continue;
        }
        if scan
            .windows
            .iter()
            .any(|window| window.contains(rowid, edited_raw, &guid))
        {
            continue;
        }

        let sent_raw: i64 = row.get(5).map_err(|_| ExportError::DatabaseRead)?;
        let from_me: bool = row.get(6).map_err(|_| ExportError::DatabaseRead)?;
        let plain_text: Option<String> = row.get(7).map_err(|_| ExportError::DatabaseRead)?;
        let attributed: Option<Vec<u8>> = row.get(8).map_err(|_| ExportError::DatabaseRead)?;
        let summary: Option<Vec<u8>> = row.get(10).map_err(|_| ExportError::DatabaseRead)?;
        let parsed_text = canonical_message_text(summary, plain_text, attributed)
            .map(|value| bounded(value, 100_000));
        let sender_rowid: Option<i64> = row.get(4).map_err(|_| ExportError::DatabaseRead)?;
        emitter.record(&Message {
            record_type: "message",
            guid: short(guid.clone()),
            chat_guid: short(
                row.get::<_, String>(3)
                    .map_err(|_| ExportError::DatabaseRead)?,
            ),
            sender: if from_me {
                None
            } else {
                sender_rowid.map(handle_source_id)
            },
            direction: if from_me { "outgoing" } else { "incoming" },
            sent_at: apple_date(sent_raw)?,
            edited_at: (edited_raw > 0)
                .then(|| apple_date(edited_raw))
                .transpose()?,
            text: parsed_text,
            edit_state: if edited_raw > 0 { "edited" } else { "original" },
            reactions: scan.reaction_index.get(&guid).cloned().unwrap_or_default(),
        })?;
        emit_attachments(database, attachment_columns, rowid, &guid, emitter)?;
        count += 1;
        if scan.progress_every > 0 && count.is_multiple_of(scan.progress_every) {
            // After the attachments, so everything up to and including this
            // message precedes the record a resumed pass will skip to.
            emitter.record(&Progress {
                record_type: "progress",
                cursor: encode_cursor(&scan.marks.cursor(scan.fingerprint, rowid))?,
            })?;
        }
    }
    Ok(count)
}

fn load_reactions(
    database: &Connection,
    message_columns: &HashSet<String>,
) -> Result<HashMap<String, Vec<Reaction>>, ExportError> {
    if !message_columns.contains("associated_message_guid")
        || !message_columns.contains("associated_message_type")
    {
        return Ok(HashMap::new());
    }
    let emoji = optional_nullable(message_columns, "associated_message_emoji");
    let handle_id = optional_integer(message_columns, "handle_id", "NULL");
    let sql = format!(
        "SELECT ROWID, guid, {handle_id} AS handle_id, associated_message_guid, \
         associated_message_type, {emoji} AS emoji \
         FROM message m WHERE associated_message_guid IS NOT NULL \
         AND associated_message_type IS NOT NULL ORDER BY ROWID"
    );
    let mut statement = database
        .prepare_cached(&sql)
        .map_err(|_| ExportError::DatabaseSchema)?;
    let mut rows = statement.query([]).map_err(|_| ExportError::DatabaseRead)?;
    let mut values: HashMap<String, Vec<Reaction>> = HashMap::new();
    while let Some(row) = rows.next().map_err(|_| ExportError::DatabaseRead)? {
        let associated: String = row.get(3).map_err(|_| ExportError::DatabaseRead)?;
        let target = canonical_target(&associated).to_owned();
        let reactions = values.entry(target).or_default();
        if reactions.len() >= 200 {
            continue;
        }
        let reaction_type: i64 = row.get(4).map_err(|_| ExportError::DatabaseRead)?;
        if !is_reaction_type(reaction_type) {
            continue;
        }
        let emoji: Option<String> = row.get(5).map_err(|_| ExportError::DatabaseRead)?;
        let guid: Option<String> = row.get(1).map_err(|_| ExportError::DatabaseRead)?;
        let rowid: i64 = row.get(0).map_err(|_| ExportError::DatabaseRead)?;
        let sender: Option<i64> = row.get(2).map_err(|_| ExportError::DatabaseRead)?;
        reactions.push(Reaction {
            source_id: short(guid.unwrap_or_else(|| format!("reaction:{rowid}"))),
            sender: sender.map(handle_source_id),
            kind: short(emoji.unwrap_or_else(|| reaction_kind(reaction_type).to_owned())),
            removed: (3000..4000).contains(&reaction_type),
        });
    }
    Ok(values)
}

fn canonical_message_text(
    summary: Option<Vec<u8>>,
    plain_text: Option<String>,
    attributed: Option<Vec<u8>>,
) -> Option<String> {
    let edited = summary
        .and_then(|bytes| PlistValue::from_reader(Cursor::new(bytes)).ok())
        .and_then(|plist| EditedMessage::from_map(&plist).ok());
    if let Some(edited) = edited {
        let tracked_parts = edited.parts.len();
        let mut parts = Vec::new();
        let mut complete = true;
        for part in edited.parts {
            match part.status {
                EditStatus::Edited => match part.edit_history.last() {
                    Some(event) => parts.push(event.text.clone()),
                    None => complete = false,
                },
                EditStatus::Unsent => {}
                EditStatus::Original => complete = false,
            }
        }
        if complete && tracked_parts > 0 {
            return if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            };
        }
    }
    plain_text.or_else(|| attributed.and_then(|body| streamtyped::parse(body).ok()))
}

fn changed_reaction_targets(
    database: &Connection,
    message_columns: &HashSet<String>,
    previous_rowid: i64,
    previous_message_date: i64,
) -> Result<HashSet<String>, ExportError> {
    if !message_columns.contains("associated_message_guid")
        || !message_columns.contains("associated_message_type")
    {
        return Ok(HashSet::new());
    }
    let mut statement = database
        .prepare(
            "SELECT associated_message_guid, associated_message_type FROM message \
             WHERE (ROWID > ?1 OR date > ?2) \
             AND associated_message_guid IS NOT NULL \
             AND associated_message_type IS NOT NULL ORDER BY ROWID",
        )
        .map_err(|_| ExportError::DatabaseSchema)?;
    let rows = statement
        .query_map([previous_rowid, previous_message_date], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|_| ExportError::DatabaseRead)?;
    let mut targets = HashSet::new();
    for value in rows {
        let (target, reaction_type) = value.map_err(|_| ExportError::DatabaseRead)?;
        if is_reaction_type(reaction_type) {
            targets.insert(canonical_target(&target).to_owned());
        }
    }
    Ok(targets)
}

fn report_unassociated_messages(
    database: &Connection,
    message_columns: &HashSet<String>,
) -> Result<(), ExportError> {
    let item_type = optional_integer(message_columns, "item_type", "0");
    let group_action = optional_integer(message_columns, "group_action_type", "0");
    let associated_type = optional_integer(message_columns, "associated_message_type", "NULL");
    let recoverable = if table_exists(database, "chat_recoverable_message_join")? {
        "AND NOT EXISTS (SELECT 1 FROM chat_recoverable_message_join r \
                         WHERE r.message_id = m.ROWID)"
    } else {
        ""
    };
    let sql = format!(
        "SELECT COUNT(*) FROM message m \
         WHERE m.guid IS NOT NULL AND m.guid != '' \
         AND COALESCE({item_type}, 0) = 0 \
         AND COALESCE({group_action}, 0) = 0 \
         AND COALESCE({associated_type}, 0) IN (0, 2, 3) \
         AND NOT EXISTS (SELECT 1 FROM chat_message_join a \
                         WHERE a.message_id = m.ROWID) {recoverable}"
    );
    let count: i64 = database
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|_| ExportError::DatabaseRead)?;
    if count > 0 {
        eprintln!("diagnostic=unassociated_canonical_messages count={count}");
    }
    Ok(())
}

fn emit_attachments<W: Write>(
    database: &Connection,
    attachment_columns: &HashSet<String>,
    message_rowid: i64,
    message_guid: &str,
    emitter: &mut Emitter<W>,
) -> Result<(), ExportError> {
    let guid = optional_nullable(attachment_columns, "guid");
    let filename = optional_nullable(attachment_columns, "filename");
    let transfer_name = optional_nullable(attachment_columns, "transfer_name");
    let mime_type = optional_nullable(attachment_columns, "mime_type");
    let total_bytes = optional_integer(attachment_columns, "total_bytes", "NULL");
    let sql = format!(
        "SELECT a.ROWID, {guid} AS guid, {filename} AS filename, \
         {transfer_name} AS transfer_name, {mime_type} AS mime_type, \
         {total_bytes} AS total_bytes \
         FROM message_attachment_join maj JOIN attachment a ON a.ROWID = maj.attachment_id \
         WHERE maj.message_id = ?1 ORDER BY a.ROWID"
    );
    let mut statement = database
        .prepare_cached(&sql)
        .map_err(|_| ExportError::DatabaseSchema)?;
    let mut rows = statement
        .query([message_rowid])
        .map_err(|_| ExportError::DatabaseRead)?;
    while let Some(row) = rows.next().map_err(|_| ExportError::DatabaseRead)? {
        let rowid: i64 = row.get(0).map_err(|_| ExportError::DatabaseRead)?;
        let guid: Option<String> = row.get(1).map_err(|_| ExportError::DatabaseRead)?;
        let filename: Option<String> = row.get(2).map_err(|_| ExportError::DatabaseRead)?;
        let transfer_name: Option<String> = row.get(3).map_err(|_| ExportError::DatabaseRead)?;
        let mime_type: Option<String> = row.get(4).map_err(|_| ExportError::DatabaseRead)?;
        let total_bytes: Option<i64> = row.get(5).map_err(|_| ExportError::DatabaseRead)?;
        let basename = transfer_name
            .as_deref()
            .and_then(safe_basename)
            .or_else(|| filename.as_deref().and_then(safe_basename));
        emitter.record(&Attachment {
            record_type: "attachment",
            source_id: short(guid.unwrap_or_else(|| format!("attachment:{rowid}"))),
            message_guid: short(message_guid.to_owned()),
            mime_type: mime_type.map(short),
            basename,
            size_bytes: total_bytes.filter(|value| *value >= 0),
            available: filename.as_deref().is_some_and(attachment_available),
        })?;
    }
    Ok(())
}

fn chat_identity_expression(columns: &HashSet<String>) -> Result<&'static str, ExportError> {
    if columns.contains("guid") && columns.contains("chat_identifier") {
        Ok("COALESCE(NULLIF(c.guid, ''), c.chat_identifier)")
    } else if columns.contains("guid") {
        Ok("c.guid")
    } else if columns.contains("chat_identifier") {
        Ok("c.chat_identifier")
    } else {
        Err(ExportError::DatabaseSchema)
    }
}

fn optional_text(columns: &HashSet<String>, column: &str, fallback: &str) -> String {
    if columns.contains(column) {
        format!("COALESCE({column}, '{fallback}')")
    } else {
        format!("'{fallback}'")
    }
}

fn optional_nullable(columns: &HashSet<String>, column: &str) -> &'static str {
    if columns.contains(column) {
        // Every caller uses one of these known schema column names.
        match column {
            "text" => "m.text",
            "attributedBody" => "m.attributedBody",
            "message_summary_info" => "m.message_summary_info",
            "associated_message_emoji" => "associated_message_emoji",
            "guid" => "a.guid",
            "filename" => "a.filename",
            "transfer_name" => "a.transfer_name",
            "mime_type" => "a.mime_type",
            _ => "NULL",
        }
    } else {
        "NULL"
    }
}

fn optional_integer(
    columns: &HashSet<String>,
    column: &str,
    fallback: &'static str,
) -> &'static str {
    if columns.contains(column) {
        match column {
            "handle_id" => "m.handle_id",
            "date_edited" => "m.date_edited",
            "item_type" => "m.item_type",
            "group_action_type" => "m.group_action_type",
            "associated_message_type" => "m.associated_message_type",
            "total_bytes" => "a.total_bytes",
            _ => fallback,
        }
    } else {
        fallback
    }
}

fn canonical_target(value: &str) -> &str {
    value.split_once('/').map_or_else(
        || value.strip_prefix("bp:").unwrap_or(value),
        |(_, guid)| guid,
    )
}

fn reaction_kind(value: i64) -> &'static str {
    match value % 1000 {
        0 => "love",
        1 => "like",
        2 => "dislike",
        3 => "laugh",
        4 => "emphasis",
        5 => "question",
        6 => "custom",
        _ => "unknown",
    }
}

fn is_canonical_association(value: Option<i64>) -> bool {
    matches!(value, None | Some(0 | 2 | 3))
}

fn is_reaction_type(value: i64) -> bool {
    value == 1000 || (2000..=2007).contains(&value) || (3000..=3007).contains(&value)
}

fn handle_source_id(rowid: i64) -> String {
    format!("handle:{rowid}")
}

fn apple_date(raw: i64) -> Result<String, ExportError> {
    let (seconds, nanos) = if raw.abs() >= NANOSECOND_THRESHOLD {
        (raw / 1_000_000_000, raw.rem_euclid(1_000_000_000) as u32)
    } else {
        (raw, 0)
    };
    let unix_seconds = seconds
        .checked_add(APPLE_EPOCH_UNIX_SECONDS)
        .ok_or(ExportError::InvalidTimestamp)?;
    DateTime::<Utc>::from_timestamp(unix_seconds, nanos)
        .ok_or(ExportError::InvalidTimestamp)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Nanos, true))
}

fn safe_basename(value: &str) -> Option<String> {
    let basename = value.rsplit(['/', '\\']).next().unwrap_or_default();
    (!basename.is_empty()).then(|| bounded(basename.to_owned(), 2_048))
}

fn attachment_available(value: &str) -> bool {
    let expanded = value
        .strip_prefix("~/")
        .and_then(|suffix| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(suffix)));
    fs::metadata(expanded.as_deref().unwrap_or_else(|| Path::new(value)))
        .is_ok_and(|metadata| metadata.is_file())
}

fn short(value: String) -> String {
    let value = bounded(value, 2_048);
    if value.is_empty() {
        "unknown".to_owned()
    } else {
        value
    }
}

fn bounded(value: String, max_chars: usize) -> String {
    match value.char_indices().nth(max_chars) {
        Some((index, _)) => value[..index].to_owned(),
        None => value,
    }
}

fn encode_cursor(cursor: &CursorV1) -> Result<String, ExportError> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| ExportError::Output)
}

fn decode_cursor(value: &str) -> Result<CursorV1, ExportError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ExportError::InvalidCursor)?;
    let cursor: CursorV1 =
        serde_json::from_slice(&bytes).map_err(|_| ExportError::InvalidCursor)?;
    if cursor.version != SCHEMA_VERSION
        || cursor.last_message_rowid < 0
        || cursor.last_edited_at < 0
        || cursor.last_message_date < 0
        || cursor.database_fingerprint.is_empty()
    {
        return Err(ExportError::InvalidCursor);
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests;
