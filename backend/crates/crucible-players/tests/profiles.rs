//! Player profile scenarios.

use crucible_players::persist::migrate;
use crucible_players::{
    CharacterProfile, NetUserId, PlayerProfile, ProfileStore, RolePriority, SqliteProfileStore,
};
use rusqlite::Connection;

/// Scenario: a new player's profile.
#[test]
fn new_player_profile() {
    let store = SqliteProfileStore::open_in_memory().unwrap();
    let user = NetUserId::new("newcomer");
    assert_eq!(store.load(&user).unwrap(), None);

    let (profile, created) = store.load_or_create(&user).unwrap();

    assert!(created);
    assert_eq!(profile, PlayerProfile::default());
    // The profile was saved. The next load finds it without creating another.
    assert_eq!(store.load(&user).unwrap(), Some(PlayerProfile::default()));
    let (again, created_again) = store.load_or_create(&user).unwrap();
    assert!(!created_again);
    assert_eq!(again, profile);
}

/// Scenario: persistence across restarts.
#[test]
fn persistence_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("players.sqlite3");
    let user = NetUserId::new("alice");

    let mut changed = PlayerProfile::default();
    changed
        .role_preferences
        .insert("captain".to_owned(), RolePriority::High);
    changed
        .role_preferences
        .insert("clown".to_owned(), RolePriority::Never);
    changed.characters.push(CharacterProfile {
        name: "Zoë".to_owned(),
        data: serde_json::json!({"age": 31, "traits": ["brave", "stubborn"]}),
    });
    changed
        .settings
        .insert("volume".to_owned(), serde_json::json!(0.5));
    {
        let store = SqliteProfileStore::open(&path).unwrap();
        store.load_or_create(&user).unwrap();
        store.save(&user, &changed).unwrap();
    } // Server stop closes the connection.

    let restarted = SqliteProfileStore::open(&path).unwrap();
    assert_eq!(restarted.load(&user).unwrap(), Some(changed));
}

#[test]
fn migrations_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("players.sqlite3");
    let user = NetUserId::new("alice");
    let first = SqliteProfileStore::open(&path).unwrap();
    first.load_or_create(&user).unwrap();
    let version = first.schema_version().unwrap();
    drop(first);

    // Reopening succeeds without replaying migrations or losing data.
    let second = SqliteProfileStore::open(&path).unwrap();
    assert_eq!(second.schema_version().unwrap(), version);
    assert!(second.load(&user).unwrap().is_some());
    let mut conn = Connection::open(&path).unwrap();
    assert_eq!(migrate(&mut conn).unwrap(), version);
    let applied: u32 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(applied, version);
}

#[test]
fn newer_schema_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("players.sqlite3");
    drop(SqliteProfileStore::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute("INSERT INTO schema_version (version) VALUES (999)", [])
        .unwrap();
    drop(conn);

    assert!(matches!(
        SqliteProfileStore::open(&path),
        Err(crucible_players::PersistError::SchemaTooNew { found: 999, .. })
    ));
}
