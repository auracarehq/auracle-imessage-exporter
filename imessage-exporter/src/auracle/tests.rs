use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde_json::Value;
use tempfile::TempDir;

use super::{ExportOptions, canonical_message_text, export_jsonl};

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
                INSERT INTO chat(ROWID, guid, chat_identifier, service_name) VALUES
                    (10, 'chat-direct', 'direct', 'iMessage'),
                    (11, 'chat-group', 'group', 'iMessage');
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
    let mut output = Vec::new();
    export_jsonl(
        &ExportOptions {
            db_path: path.to_owned(),
            cursor,
        },
        &mut output,
    )
    .expect("export succeeds");
    String::from_utf8(output).expect("JSONL is UTF-8")
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
