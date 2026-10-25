CREATE TABLE mod_catalogue_records (
    provider_name TEXT NOT NULL,
    provider_record_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    imported_at TEXT NOT NULL,
    PRIMARY KEY (provider_name, provider_record_id)
);

CREATE INDEX mod_catalogue_records_imported_at
    ON mod_catalogue_records (imported_at, provider_name, provider_record_id);
