use rusqlite::Connection;

pub(crate) fn migrate_activation_deadlines(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|error| error.to_string())?;
    tx.execute_batch(
        "ALTER TABLE delegation_children ADD COLUMN admitted_at_ms INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE delegation_children ADD COLUMN deadline_stop INTEGER NOT NULL DEFAULT 0 CHECK(deadline_stop IN (0,1));
         UPDATE delegation_children SET admitted_at_ms=CAST(unixepoch('subsec')*1000 AS INTEGER);
         CREATE INDEX delegation_deadline ON delegation_children(admitted_at_ms) WHERE active=1;
         PRAGMA user_version=16;",
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())
}

pub(crate) fn migrate_followup_delivery(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch(
        "ALTER TABLE delegation_followup_queue ADD COLUMN preferred_run TEXT;
         ALTER TABLE delegation_followup_queue ADD COLUMN delivery_run TEXT;
         ALTER TABLE delegation_followup_queue ADD COLUMN delivery_state TEXT
             CHECK((delivery_run IS NULL AND delivery_state IS NULL) OR
                   (delivery_run IS NOT NULL AND delivery_state IS NOT NULL
                       AND delivery_state IN ('claimed','delivered','unknown')
                       AND activation_id IS NULL AND cancelled=0));
         DROP INDEX delegation_followup_child;
         CREATE INDEX delegation_followup_child ON delegation_followup_queue(delegation_id,sequence)
             WHERE cancelled=0 AND activation_id IS NULL AND delivery_run IS NULL;
         PRAGMA user_version=15;",
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn migrate_questions(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch(
        "CREATE TABLE delegation_questions (
            id TEXT PRIMARY KEY,
            delegation_id TEXT NOT NULL REFERENCES delegation_children(id),
            activation_id TEXT NOT NULL,
            question_json TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'pending'
                CHECK(state IN ('pending','delivering','delivered','expired','unknown')),
            operation_id TEXT,
            answered_by TEXT,
            answer TEXT
         );
         CREATE INDEX delegation_questions_child ON delegation_questions(delegation_id,activation_id);
         PRAGMA user_version=14;",
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn migrate_followup_promotion(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch(
        "ALTER TABLE delegation_followup_queue ADD COLUMN activation_id TEXT;
         DROP INDEX delegation_followup_child;
         CREATE INDEX delegation_followup_child ON delegation_followup_queue(delegation_id,sequence)
             WHERE cancelled=0 AND activation_id IS NULL;
         PRAGMA user_version=13;",
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) fn remove_from_legacy_fixture(db: &Connection) {
    db.execute_batch("DROP TABLE delegation_questions; DROP TABLE delegation_followup_queue; DROP TABLE delegation_stopped_trees; DROP TABLE delegation_terminal_snapshots; DROP TABLE delegation_wake_state; DROP TABLE delegation_notices; DROP TABLE delegation_consumers; DROP TABLE delegation_outbox; DROP TABLE delegation_results; DROP TABLE delegation_operations; DROP TABLE delegation_children;").unwrap();
}

pub(crate) fn migrate_followup_queue(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch(
        "CREATE TABLE delegation_followup_queue (
            sequence INTEGER PRIMARY KEY,
            work_item_id TEXT NOT NULL UNIQUE,
            caller_session TEXT NOT NULL REFERENCES sessions(id),
            operation_id TEXT NOT NULL,
            delegation_id TEXT NOT NULL REFERENCES delegation_children(id),
            request_json TEXT NOT NULL,
            cancelled INTEGER NOT NULL DEFAULT 0 CHECK(cancelled IN (0,1)),
            UNIQUE(caller_session,operation_id)
        );
        CREATE INDEX delegation_followup_child ON delegation_followup_queue(delegation_id,sequence) WHERE cancelled=0;
        PRAGMA user_version=12;",
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn migrate_tree_stop(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch(
        "CREATE TABLE delegation_stopped_trees (
            root_session TEXT PRIMARY KEY REFERENCES sessions(id)
        );
        PRAGMA user_version=11;",
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn migrate_terminal_snapshots(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch(
        "CREATE TABLE delegation_terminal_snapshots (
            activation_id TEXT PRIMARY KEY REFERENCES delegation_results(activation_id),
            snapshot_json TEXT NOT NULL
        );
        INSERT INTO delegation_terminal_snapshots
            SELECT r.activation_id,
                (SELECT c.snapshot_json FROM delegation_children c WHERE c.activation_id=r.activation_id)
            FROM delegation_results r;
        PRAGMA user_version=9;",
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn migrate_operation_activations(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch(
        "ALTER TABLE delegation_operations ADD COLUMN activation_id TEXT NOT NULL DEFAULT '';
        UPDATE delegation_operations SET activation_id=(
            SELECT c.activation_id FROM delegation_children c WHERE c.id=delegation_operations.delegation_id
        );
        PRAGMA user_version=10;",
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn migrate(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch(
        "CREATE TABLE delegation_children (
            id TEXT PRIMARY KEY,
            parent_session TEXT NOT NULL REFERENCES sessions(id),
            root_session TEXT NOT NULL REFERENCES sessions(id),
            activation_id TEXT NOT NULL UNIQUE,
            snapshot_json TEXT NOT NULL,
            active INTEGER NOT NULL CHECK(active IN (0,1))
        );
        CREATE INDEX delegation_root ON delegation_children(root_session,active);
        CREATE TABLE delegation_operations (
            caller_session TEXT NOT NULL REFERENCES sessions(id),
            operation_id TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            delegation_id TEXT NOT NULL REFERENCES delegation_children(id),
            PRIMARY KEY(caller_session,operation_id)
        );
        CREATE TABLE delegation_results (
            activation_id TEXT PRIMARY KEY,
            delegation_id TEXT NOT NULL REFERENCES delegation_children(id),
            completion_json TEXT NOT NULL
        );
        CREATE TABLE delegation_outbox (
            activation_id TEXT PRIMARY KEY REFERENCES delegation_results(activation_id),
            parent_session TEXT NOT NULL REFERENCES sessions(id),
            acknowledged INTEGER NOT NULL DEFAULT 0 CHECK(acknowledged IN (0,1))
        );
        CREATE TABLE delegation_consumers (
            session_id TEXT PRIMARY KEY REFERENCES sessions(id),
            owner TEXT NOT NULL,
            generation INTEGER NOT NULL
        );
        CREATE TABLE delegation_notices (
            result_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id),
            notice_json TEXT NOT NULL,
            event_seq INTEGER NOT NULL,
            bound_run TEXT
        );
        CREATE TABLE delegation_wake_state (
            session_id TEXT PRIMARY KEY REFERENCES sessions(id),
            generation INTEGER NOT NULL,
            stopped INTEGER NOT NULL CHECK(stopped IN (0,1))
        );
        PRAGMA user_version=8;",
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
