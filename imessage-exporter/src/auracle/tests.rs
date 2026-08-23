use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde_json::Value;
use tempfile::TempDir;

use super::{
    DEFAULT_PROGRESS_EVERY, ExportOptions, canonical_message_text, decode_cursor, export_jsonl,
};

struct Fixture {
    _directory: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn rich() -> Self {
        let directory = tempfile::tempdir().expect("fixture directory");
        let path = directory.path().join("chat.db");
        let database = Connection::open(&path).expect("fixture database");
        database
            .execute_batch(
                "
                CREATE TABLE handle (id TEXT, service TEXT);
                CREATE TABLE chat (
                    guid TEXT, chat_identifier TEXT, service_name TEXT, display_name TEXT
                );
                CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
                CREATE TABLE message (
                    guid TEXT, text TEXT, attributedBody BLOB, service TEXT,
                    handle_id INTEGER, date INTEGER, is_from_me INTEGER,
                    item_type INTEGER DEFAULT 0, group_action_type INTEGER DEFAULT 0,
                    associated_message_guid TEXT, associated_message_type INTEGER,
                    date_edited INTEGER DEFAULT 0, associated_message_emoji TEXT,
                    message_summary_info BLOB
                );
                CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
                CREATE TABLE chat_recoverable_message_join (chat_id INTEGER, message_id INTEGER);
                CREATE TABLE attachment (
                    guid TEXT, filename TEXT, transfer_name TEXT, mime_type TEXT,
                    total_bytes INTEGER
                );
                CREATE TABLE message_attachment_join (attachment_id INTEGER, message_id INTEGER);

                INSERT INTO handle(ROWID, id, service) VALUES
                    (1, '+15550100001', 'iMessage'),
                    (2, 'friend@example.test', 'iMessage');
                INSERT INTO chat(ROWID, guid, chat_identifier, service_name, display_name) VALUES
                    (10, 'chat-direct', 'direct', 'iMessage', NULL),
                    (11, 'chat-group', 'group', 'iMessage', '  Sunday Five-a-side ');
                INSERT INTO chat_handle_join(chat_id, handle_id) VALUES
                    (10, 1), (11, 1), (11, 2);

                INSERT INTO message(
                    ROWID, guid, text, service, handle_id, date, is_from_me
                ) VALUES
                    (100, 'GUID-M1', 'direct incoming', 'iMessage', 1, 1, 0),
                    (101, 'GUID-M2', 'group outgoing', 'iMessage', NULL, 2, 1);
                INSERT INTO message(
                    ROWID, guid, text, service, handle_id, date, is_from_me,
                    associated_message_guid, associated_message_type
                ) VALUES
                    (102, 'GUID-R1', NULL, 'iMessage', 2, 3, 0, 'p:0/GUID-M1', 2001);
                INSERT INTO message(
                    ROWID, guid, text, service, handle_id, date, is_from_me,
                    item_type, group_action_type
                ) VALUES
                    (103, 'GUID-SERVICE', 'joined the chat', 'iMessage', 1, 4, 0, 1, 1);
                INSERT INTO message(
                    ROWID, guid, text, service, handle_id, date, is_from_me
                ) VALUES
                    (104, 'GUID-M3', NULL, 'iMessage', 2, 5, 0);
                INSERT INTO message(
                    ROWID, guid, text, service, handle_id, date, is_from_me, date_edited
                ) VALUES
                    (105, 'GUID-EDITED', NULL, 'iMessage', 1, 6, 0, 690513494000000000);
                INSERT INTO message(
                    ROWID, guid, text, service, handle_id, date, is_from_me,
                    associated_message_guid, associated_message_type
                ) VALUES
                    (106, 'GUID-APP-2', 'app payload two', 'iMessage', 1, 7, 0,
                     'bp:GUID-M1', 2),
                    (107, 'GUID-APP-3', 'app payload three', 'iMessage', 2, 8, 0,
                     'bp:GUID-M1', 3),
                    (108, 'GUID-POLL', 'poll vote', 'iMessage', 2, 9, 0,
                     'bp:GUID-M1', 4000);
                INSERT INTO message(
                    ROWID, guid, text, service, handle_id, date, is_from_me
                ) VALUES
                    (109, 'GUID-RECOVERED', 'recently deleted', 'iMessage', 1, 10, 0);
                INSERT INTO chat_message_join(chat_id, message_id) VALUES
                    (10, 100), (11, 101), (10, 102), (11, 103), (11, 104), (10, 105),
                    (10, 106), (11, 107), (10, 108);
                INSERT INTO chat_recoverable_message_join(chat_id, message_id) VALUES (10, 109);

                INSERT INTO attachment(
                    ROWID, guid, filename, transfer_name, mime_type, total_bytes
                ) VALUES
                    (200, 'ATTACHMENT-1', '/private/does-not-exist/photo.jpg',
                     'safe-photo.jpg', 'image/jpeg', 1234);
                INSERT INTO message_attachment_join(attachment_id, message_id) VALUES (200, 101);
                ",
            )
            .expect("rich schema");
        let body = [
            vec![0x01, 0x2b, 0x06],
            b"attributed-body text".to_vec(),
            vec![0x86, 0x84, 0x00],
        ]
        .concat();
        database
            .execute(
                "UPDATE message SET attributedBody = ?1 WHERE ROWID = 104",
                [body],
            )
            .expect("attributed body");
        database
            .execute(
                "UPDATE message SET message_summary_info = ?1 WHERE ROWID = 105",
                [include_bytes!(
                    "../../../imessage-database/test_data/edited_message/Edited.plist"
                )
                .as_slice()],
            )
            .expect("modern edited-message summary");
        drop(database);
        Self {
            _directory: directory,
            path,
        }
    }

    fn legacy_schema() -> Self {
        let directory = tempfile::tempdir().expect("fixture directory");
        let path = directory.path().join("chat.db");
        let database = Connection::open(&path).expect("fixture database");
        database
            .execute_batch(
                "
                CREATE TABLE handle (id TEXT);
                CREATE TABLE chat (chat_identifier TEXT);
                CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
                CREATE TABLE message (guid TEXT, date INTEGER, is_from_me INTEGER);
                CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
                CREATE TABLE attachment (filename TEXT);
                CREATE TABLE message_attachment_join (attachment_id INTEGER, message_id INTEGER);
                INSERT INTO handle(ROWID, id) VALUES (1, '+15550100003');
                INSERT INTO chat(ROWID, chat_identifier) VALUES (1, 'legacy-chat');
                INSERT INTO chat_handle_join(chat_id, handle_id) VALUES (1, 1);
                INSERT INTO message(ROWID, guid, date, is_from_me) VALUES (1, 'LEGACY-M1', 1, 0);
                INSERT INTO chat_message_join(chat_id, message_id) VALUES (1, 1);
                ",
            )
            .expect("legacy schema");
        drop(database);
        Self {
            _directory: directory,
            path,
        }
    }
}

fn run(path: &Path, cursor: Option<String>) -> String {
    run_with(path, cursor, None, DEFAULT_PROGRESS_EVERY)
}

fn run_with(
    path: &Path,
    cursor: Option<String>,
    resume: Option<String>,
    progress_every: u64,
) -> String {
    let mut output = Vec::new();
    export_jsonl(
        &ExportOptions {
            db_path: path.to_owned(),
            cursor,
            resume,
            progress_every,
        },
        &mut output,
    )
    .expect("export succeeds");
    String::from_utf8(output).expect("JSONL is UTF-8")
}

fn message_guids(output: &str) -> Vec<String> {
    records(output)
        .into_iter()
        .filter(|record| record["type"] == "message")
        .map(|record| record["guid"].as_str().expect("guid").to_owned())
        .collect()
}

fn progress_cursors(output: &str) -> Vec<String> {
    records(output)
        .into_iter()
        .filter(|record| record["type"] == "progress")
        .map(|record| record["cursor"].as_str().expect("cursor").to_owned())
        .collect()
}

fn add_message(database: &Connection, rowid: i64, guid: &str, chat: i64) {
    database
        .execute(
            "INSERT INTO message(ROWID, guid, text, service, handle_id, date, is_from_me) \
             VALUES (?1, ?2, 'later canonical', 'iMessage', 1, ?1, 0)",
            params![rowid, guid],
        )
        .expect("new message");
    database
        .execute(
            "INSERT INTO chat_message_join(chat_id, message_id) VALUES (?1, ?2)",
            params![chat, rowid],
        )
        .expect("message chat");
}

fn edit_message(database: &Connection, rowid: i64) {
    database
        .execute(
            "UPDATE message SET text = 'edited canonical', \
             date_edited = 800000000000000000 WHERE ROWID = ?1",
            [rowid],
        )
        .expect("edit message");
}

fn records(output: &str) -> Vec<Value> {
    output
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid JSON"))
        .collect()
}

fn assert_schema(output: &str) {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../docs/auracle-imessage-jsonl-v1.schema.json"
    ))
    .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compiled schema");
    for (index, record) in records(output).iter().enumerate() {
        if let Err(error) = validator.validate(record) {
            panic!("record {index} failed JSON Schema validation: {error}");
        }
    }
}

fn checkpoint_cursor(output: &str) -> String {
    records(output)
        .into_iter()
        .find(|record| record["type"] == "checkpoint")
        .and_then(|record| record["cursor"].as_str().map(str::to_owned))
        .expect("checkpoint cursor")
}

#[test]
fn all_synthetic_fixtures_validate_against_the_normative_schema() {
    for fixture in [Fixture::rich(), Fixture::legacy_schema()] {
        assert_schema(&run(&fixture.path, None));
    }
}

#[test]
fn rich_fixture_covers_relationships_edits_reactions_services_bodies_and_attachments() {
    let fixture = Fixture::rich();
    let output = run(&fixture.path, None);
    let values = records(&output);

    let chats: Vec<_> = values
        .iter()
        .filter(|record| record["type"] == "chat")
        .collect();
    assert_eq!(chats.len(), 2);
    assert!(chats.iter().any(|chat| chat["kind"] == "direct"));
    assert!(chats.iter().any(|chat| chat["kind"] == "group"));

    let messages: Vec<_> = values
        .iter()
        .filter(|record| record["type"] == "message")
        .collect();
    assert_eq!(messages.len(), 7);
    assert!(!messages.iter().any(|message| message["guid"] == "GUID-R1"));
    assert!(
        !messages
            .iter()
            .any(|message| message["guid"] == "GUID-SERVICE")
    );
    assert!(
        !messages
            .iter()
            .any(|message| message["guid"] == "GUID-POLL")
    );
    assert!(
        messages
            .iter()
            .any(|message| message["guid"] == "GUID-APP-2")
    );
    assert!(
        messages
            .iter()
            .any(|message| message["guid"] == "GUID-APP-3")
    );
    assert!(messages.iter().any(|message| {
        message["guid"] == "GUID-RECOVERED" && message["chat_guid"] == "chat-direct"
    }));
    assert!(messages.iter().any(|message| {
        message["guid"] == "GUID-M1"
            && message["direction"] == "incoming"
            && message["sender"] == "handle:1"
            && message["reactions"]
                .as_array()
                .is_some_and(|value| value.len() == 1)
    }));
    assert!(messages.iter().any(|message| {
        message["guid"] == "GUID-M2"
            && message["direction"] == "outgoing"
            && message["sender"].is_null()
    }));
    assert!(messages.iter().any(|message| {
        message["guid"] == "GUID-M3" && message["text"] == "attributed-body text"
    }));
    assert!(messages.iter().any(|message| {
        message["guid"] == "GUID-EDITED"
            && message["text"] == "Edited message"
            && message["edit_state"] == "edited"
    }));

    let attachment = values
        .iter()
        .find(|record| record["type"] == "attachment")
        .expect("attachment record");
    assert_eq!(attachment["basename"], "safe-photo.jpg");
    assert_eq!(attachment["available"], false);
    assert!(!output.contains("/private/does-not-exist"));
}

#[test]
fn output_is_deterministic_and_matches_the_golden_fixture() {
    let fixture = Fixture::rich();
    let first = run(&fixture.path, None);
    let second = run(&fixture.path, None);
    assert_eq!(first, second);
    assert_eq!(first, include_str!("../../../test_data/auracle/rich.jsonl"));
}

#[test]
fn cursor_resume_emits_new_edits_and_reaction_updates_exactly_once() {
    let fixture = Fixture::rich();
    let initial = run(&fixture.path, None);
    let cursor = checkpoint_cursor(&initial);
    let database = Connection::open(&fixture.path).expect("fixture database");
    database
        .execute(
            "UPDATE message SET text = 'edited canonical', date_edited = 800000000000000000 WHERE ROWID = 100",
            [],
        )
        .expect("edit message");
    database
        .execute(
            "INSERT INTO message(
                ROWID, guid, text, service, handle_id, date, is_from_me,
                associated_message_guid, associated_message_type
             ) VALUES (110, 'GUID-R2', NULL, 'iMessage', 1, 11, 0, 'bp:GUID-M2', 2000)",
            [],
        )
        .expect("new reaction");
    database
        .execute(
            "INSERT INTO message(
                ROWID, guid, text, service, handle_id, date, is_from_me
             ) VALUES (111, 'GUID-M4', 'new canonical', 'iMessage', 1, 12, 0)",
            [],
        )
        .expect("new message");
    database
        .execute(
            "INSERT INTO chat_message_join(chat_id, message_id) VALUES (11, 110), (10, 111)",
            [],
        )
        .expect("message chats");
    drop(database);

    let resumed = run(&fixture.path, Some(cursor.clone()));
    let replayed = run(&fixture.path, Some(cursor));
    assert_eq!(resumed, replayed, "retrying a cursor is idempotent");
    assert_schema(&resumed);
    let message_guids: Vec<_> = records(&resumed)
        .into_iter()
        .filter(|record| record["type"] == "message")
        .map(|record| record["guid"].as_str().expect("guid").to_owned())
        .collect();
    assert_eq!(message_guids, ["GUID-M1", "GUID-M2", "GUID-M4"]);

    let next_cursor = checkpoint_cursor(&resumed);
    let no_changes = records(&run(&fixture.path, Some(next_cursor)));
    assert_eq!(
        no_changes
            .iter()
            .filter(|record| record["type"] == "message")
            .count(),
        0
    );
}

#[test]
fn cursor_resume_observes_an_in_place_reaction_change() {
    let fixture = Fixture::rich();
    let cursor = checkpoint_cursor(&run(&fixture.path, None));
    let database = Connection::open(&fixture.path).expect("fixture database");
    database
        .execute(
            "UPDATE message SET associated_message_type = 3001, date = 20 WHERE ROWID = 102",
            [],
        )
        .expect("remove reaction in place");
    drop(database);

    let resumed = run(&fixture.path, Some(cursor));
    assert_schema(&resumed);
    let messages: Vec<_> = records(&resumed)
        .into_iter()
        .filter(|record| record["type"] == "message")
        .collect();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["guid"], "GUID-M1");
    assert_eq!(messages[0]["reactions"][0]["removed"], true);
}

#[test]
fn manifest_detects_a_changed_database_identity() {
    let fixture = Fixture::rich();
    let before_output = run(&fixture.path, None);
    let cursor = checkpoint_cursor(&before_output);
    let before = records(&before_output);
    let database = Connection::open(&fixture.path).expect("fixture database");
    database
        .execute(
            "UPDATE message SET guid = 'REPLACED-M1' WHERE ROWID = 100",
            params![],
        )
        .expect("replace identity");
    drop(database);
    let after = records(&run(&fixture.path, Some(cursor)));
    assert_ne!(
        before[0]["database_fingerprint"],
        after[0]["database_fingerprint"]
    );
    assert_eq!(after[0]["export_mode"], "full");
    assert_eq!(
        after
            .iter()
            .filter(|record| record["type"] == "message")
            .count(),
        7
    );
}

#[test]
fn fully_unsent_modern_message_does_not_fall_back_to_deleted_text() {
    let summary =
        include_bytes!("../../../imessage-database/test_data/edited_message/Deleted.plist")
            .to_vec();
    assert_eq!(
        canonical_message_text(Some(summary), Some("deleted private text".to_owned()), None),
        None
    );
}

#[test]
fn progress_records_mark_resumable_positions_and_count_as_records() {
    let fixture = Fixture::rich();
    let output = run_with(&fixture.path, None, None, 3);
    assert_schema(&output);
    let values = records(&output);
    let cursors = progress_cursors(&output);
    // Seven messages stream; a mark lands after the third and the sixth.
    assert_eq!(cursors.len(), 2);
    let third = values
        .iter()
        .filter(|record| record["type"] == "message")
        .nth(2)
        .expect("third message");
    assert_eq!(third["guid"], "GUID-M3");
    let mark = decode_cursor(&cursors[0]).expect("progress cursor decodes");
    assert_eq!(
        mark.last_message_rowid, 104,
        "the mark is the row just streamed"
    );
    let checkpoint = decode_cursor(&checkpoint_cursor(&output)).expect("checkpoint cursor");
    assert_eq!(mark.database_fingerprint, checkpoint.database_fingerprint);
    assert_eq!(mark.last_edited_at, checkpoint.last_edited_at);
    assert_eq!(mark.last_message_date, checkpoint.last_message_date);
    assert!(mark.last_message_rowid < checkpoint.last_message_rowid);
    // The marks are records like any other: the consumer uploads and counts
    // them, so the checkpoint's total has to include them.
    let total = values
        .iter()
        .find(|record| record["type"] == "checkpoint")
        .map(|record| record["totals"]["records"].as_u64().expect("records total"))
        .expect("checkpoint");
    assert_eq!(total, values.len() as u64);
    // The position of a mark in the stream is the position the record claims.
    let mark_index = values
        .iter()
        .position(|record| record["type"] == "progress")
        .expect("first mark");
    assert!(
        values[..mark_index]
            .iter()
            .filter(|record| record["type"] == "message")
            .count()
            == 3
    );
}

#[test]
fn the_default_interval_leaves_a_short_pass_and_its_golden_output_untouched() {
    let fixture = Fixture::rich();
    let output = run(&fixture.path, None);
    assert!(progress_cursors(&output).is_empty());
    assert_eq!(
        output,
        include_str!("../../../test_data/auracle/rich.jsonl")
    );
}

#[test]
fn resuming_from_a_progress_cursor_skips_what_the_earlier_pass_carried() {
    let fixture = Fixture::rich();
    let first_pass = run_with(&fixture.path, None, None, 3);
    let mark = progress_cursors(&first_pass).remove(0);
    let database = Connection::open(&fixture.path).expect("fixture database");
    edit_message(&database, 100);
    add_message(&database, 111, "GUID-M4", 10);
    drop(database);

    let resumed = run_with(&fixture.path, None, Some(mark.clone()), 0);
    let replayed = run_with(&fixture.path, None, Some(mark), 0);
    assert_eq!(resumed, replayed, "resuming twice streams the same bytes");
    assert_schema(&resumed);
    let values = records(&resumed);
    // The resumed stream stands on its own: manifest, every handle, every chat.
    assert_eq!(values[0]["type"], "manifest");
    assert_eq!(values[0]["export_mode"], "full");
    assert_eq!(values.iter().filter(|r| r["type"] == "handle").count(), 2);
    assert_eq!(values.iter().filter(|r| r["type"] == "chat").count(), 2);
    // Then only what the earlier pass had not carried -- the messages after
    // the mark, the new message -- plus the one edited since it read the
    // database. GUID-M2 and GUID-M3 streamed before the mark and stay out.
    assert_eq!(
        message_guids(&resumed),
        [
            "GUID-M1",
            "GUID-EDITED",
            "GUID-APP-2",
            "GUID-APP-3",
            "GUID-RECOVERED",
            "GUID-M4",
        ]
    );
    assert!(resumed.contains("edited canonical"));
    // Its checkpoint is a real cursor: the next incremental pass is empty.
    let next = run(&fixture.path, Some(checkpoint_cursor(&resumed)));
    assert_eq!(records(&next)[0]["export_mode"], "incremental");
    assert!(message_guids(&next).is_empty());
}

#[test]
fn resuming_an_incremental_pass_keeps_both_windows_closed() {
    let fixture = Fixture::rich();
    let baseline = checkpoint_cursor(&run(&fixture.path, None));
    let database = Connection::open(&fixture.path).expect("fixture database");
    for (rowid, guid) in [
        (111, "GUID-N1"),
        (112, "GUID-N2"),
        (113, "GUID-N3"),
        (114, "GUID-N4"),
    ] {
        add_message(&database, rowid, guid, 10);
    }
    drop(database);

    let incremental = run_with(&fixture.path, Some(baseline.clone()), None, 2);
    assert_eq!(records(&incremental)[0]["export_mode"], "incremental");
    assert_eq!(
        message_guids(&incremental),
        ["GUID-N1", "GUID-N2", "GUID-N3", "GUID-N4"]
    );
    let mark = progress_cursors(&incremental).remove(0);
    assert_eq!(decode_cursor(&mark).expect("mark").last_message_rowid, 112);

    let database = Connection::open(&fixture.path).expect("fixture database");
    add_message(&database, 115, "GUID-N5", 10);
    edit_message(&database, 100);
    drop(database);

    let resumed = run_with(&fixture.path, Some(baseline), Some(mark), 0);
    assert_eq!(records(&resumed)[0]["export_mode"], "incremental");
    // Outside the baseline AND outside the mark: the edit to an old message,
    // the two new messages the first pass never reached, and the newest one.
    // Inside either window -- the untouched archive, GUID-N1 and GUID-N2 --
    // stays out.
    assert_eq!(
        message_guids(&resumed),
        ["GUID-M1", "GUID-N3", "GUID-N4", "GUID-N5"]
    );
}

#[test]
fn a_progress_cursor_from_another_database_is_ignored_not_obeyed() {
    let fixture = Fixture::rich();
    let mark = progress_cursors(&run_with(&fixture.path, None, None, 3)).remove(0);
    let database = Connection::open(&fixture.path).expect("fixture database");
    database
        .execute(
            "UPDATE message SET guid = 'REPLACED-M1' WHERE ROWID = 100",
            params![],
        )
        .expect("replace identity");
    drop(database);

    let output = run_with(&fixture.path, None, Some(mark), 0);
    let values = records(&output);
    assert_eq!(values[0]["export_mode"], "full");
    assert_eq!(message_guids(&output).len(), 7);
    assert_eq!(message_guids(&output)[0], "REPLACED-M1");
}

#[test]
fn a_malformed_resume_cursor_is_refused_like_a_malformed_cursor() {
    let fixture = Fixture::rich();
    let error = export_jsonl(
        &ExportOptions {
            db_path: fixture.path.clone(),
            cursor: None,
            resume: Some("not-a-cursor".to_owned()),
            progress_every: 0,
        },
        &mut Vec::new(),
    )
    .expect_err("a malformed resume cursor cannot be obeyed");
    assert_eq!(error.to_string(), "invalid_cursor");
}

#[test]
fn a_group_carries_the_name_its_member_gave_it_and_a_direct_chat_carries_none() {
    let fixture = Fixture::rich();
    let output = run(&fixture.path, None);
    let chats: Vec<Value> = records(&output)
        .into_iter()
        .filter(|record| record["type"] == "chat")
        .collect();
    let by_guid = |guid: &str| {
        chats
            .iter()
            .find(|chat| chat["guid"] == guid)
            .cloned()
            .expect("chat record")
    };
    assert_eq!(by_guid("chat-group")["display_name"], "Sunday Five-a-side");
    assert_eq!(by_guid("chat-direct")["display_name"], Value::Null);
    assert_schema(&output);
}

#[test]
fn a_database_without_a_display_name_column_still_exports_chats() {
    let fixture = Fixture::legacy_schema();
    let output = run(&fixture.path, None);
    let chat = records(&output)
        .into_iter()
        .find(|record| record["type"] == "chat")
        .expect("chat record");
    assert_eq!(chat["display_name"], Value::Null);
}
