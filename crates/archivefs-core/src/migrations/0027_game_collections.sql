-- Migration 0027: Favourites and user-created game collections.
--
-- Additive only: no existing table or row changes. Favourites is the one
-- reserved collection, so both features share one model. Membership is user
-- data, not evidence: it says nothing about a game's identity.
--
-- game_collection_items deliberately has no foreign key to archives. Rows can
-- leave the catalogue (Forget, Discovery Undo) and come back, and a membership
-- must outlive that instead of being deleted or blocking the delete. An archive
-- id alone is not an identity because deleted ids can be allocated again, so
-- each item also records the row's created_at, which is part of its key, and
-- its location; see
-- database/collections.rs for how an item is resolved.
--
-- AUTOINCREMENT: a deleted collection's id is never given to another, so a stale
-- window or queued click cannot act on a collection the user never saw.
CREATE TABLE game_collections (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL CHECK (kind IN ('favourites','user')),
    name        TEXT NOT NULL,
    name_key    TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX game_collections_name_key ON game_collections(name_key);
CREATE UNIQUE INDEX game_collections_one_favourites ON game_collections(kind)
    WHERE kind = 'favourites';
INSERT INTO game_collections (kind, name, name_key, created_at, updated_at)
VALUES ('favourites', 'Favourites', 'favourites',
        strftime('%Y-%m-%dT%H:%M:%SZ','now'), strftime('%Y-%m-%dT%H:%M:%SZ','now'));

CREATE TABLE game_collection_items (
    collection_id      INTEGER NOT NULL REFERENCES game_collections(id) ON DELETE CASCADE,
    archive_id         INTEGER NOT NULL,
    archive_created_at TEXT NOT NULL,
    source_folder_id   INTEGER NOT NULL,
    relative_path      BLOB NOT NULL,
    absolute_path      BLOB NOT NULL,
    size_bytes         INTEGER,
    title              TEXT NOT NULL,
    platform           TEXT,
    added_at           TEXT NOT NULL,
    PRIMARY KEY (collection_id, archive_id, archive_created_at)
);
CREATE INDEX game_collection_items_archive ON game_collection_items(archive_id);
