-- Historical inventories retain NULL: a version label alone is not a digest.
ALTER TABLE dat_expected_inventory_meta ADD COLUMN catalogue_sha256 TEXT;
