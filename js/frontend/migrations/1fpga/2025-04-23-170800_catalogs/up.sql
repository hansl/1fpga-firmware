-- Delete all data from the entire database.
DELETE
FROM cores;
DELETE
FROM cores_systems;
DELETE
FROM cores_tags;

DELETE
from games;
DELETE
from games_identification;
DELETE
from games_identification_files;
DELETE
from savestates;
DELETE
from screenshots;
DELETE
from systems;
DELETE
from systems_tags;

DELETE
from user_cores;
DELETE
from user_games;

DELETE
FROM catalogs_binary;
DELETE
FROM catalogs;

ALTER TABLE catalogs
    ADD COLUMN json JSONB;

ALTER TABLE catalogs
    DROP COLUMN last_updated;

ALTER TABLE games
    DROP COLUMN games_id;

ALTER TABLE systems
    DROP COLUMN description;

DROP TABLE catalog_binaries;
DROP TABLE cores_systems;
