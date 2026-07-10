// The 1fpga database schema, applied at boot when the database is
// fresh (PRAGMA user_version = 0).
//
// This is the old frontend's schema verbatim (initial + playlists +
// sessions migrations concatenated) so catalog data, save states and
// play sessions land in the same shape when those services are
// ported. Version bumps append ALTER/CREATE statements to MIGRATIONS
// below — each entry runs once, in order, and user_version records
// how far this database has advanced.

/** Applied in order; user_version = number of entries applied. */
export const MIGRATIONS: string[] = [
  // -- v1: initial schema (old frontend 0000-00-00-000000_initial) --
  `
CREATE TABLE Users
(
    id        INTEGER PRIMARY KEY,
    username  VARCHAR(255) NOT NULL UNIQUE,
    password  VARCHAR(255),
    createdAt TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    admin     BOOLEAN      NOT NULL DEFAULT FALSE
);

CREATE TABLE UserStorage
(
    id        INTEGER PRIMARY KEY,
    usersId   INTEGER      NOT NULL REFERENCES Users (id),
    key       VARCHAR(255) NOT NULL,
    value     JSON         NOT NULL,
    updatedAt TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT userStorageUsersIdKey UNIQUE (usersId, key)
);

CREATE TABLE GlobalStorage
(
    id        INTEGER PRIMARY KEY,
    key       VARCHAR(255) NOT NULL UNIQUE,
    value     JSON         NOT NULL,
    updatedAt TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE UserCores
(
    id           INTEGER PRIMARY KEY,
    usersId      INTEGER   NOT NULL REFERENCES Users (id),
    coresId      INTEGER   NOT NULL REFERENCES Cores (id),
    favorite     BOOLEAN   NOT NULL DEFAULT FALSE,
    lastPlayedAt TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE Games
(
    id        INTEGER PRIMARY KEY,
    name      TEXT,
    coresId   INTEGER REFERENCES Cores (id),
    systemsId INTEGER REFERENCES Systems (id),
    path      TEXT,
    size      INTEGER,
    sha256    VARCHAR(64),
    CONSTRAINT coreHasName
        CHECK ( (coresId IS NOT NULL AND name IS NOT NULL)
            OR (coresId IS NULL) ),
    CONSTRAINT pathSizeSha256
        CHECK ( (path IS NOT NULL AND size IS NOT NULL AND sha256 IS NOT NULL)
            OR (path IS NULL AND size IS NULL AND sha256 IS NULL) )
);

CREATE TABLE UserGames
(
    id           INTEGER PRIMARY KEY,
    usersId      INTEGER NOT NULL REFERENCES Users (id),
    gamesId      INTEGER NOT NULL REFERENCES Games (id),
    coresId      INTEGER REFERENCES Cores (id),
    favorite     BOOLEAN NOT NULL DEFAULT FALSE,
    lastPlayedAt TIMESTAMP        DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT userGamesUsersIdGamesId UNIQUE (usersId, gamesId)
);

CREATE TABLE Savestates
(
    id             INTEGER PRIMARY KEY,
    coresId        INTEGER   NOT NULL REFERENCES Cores (id),
    gamesId        INTEGER   NOT NULL REFERENCES UserGames (id),
    usersId        INTEGER   NOT NULL,
    statePath      TEXT      NOT NULL,
    screenshotPath TEXT      NOT NULL,
    createdAt      TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE Regions
(
    id   INTEGER PRIMARY KEY,
    name VARCHAR(255) NOT NULL UNIQUE
);

CREATE TABLE GamesRegions
(
    gamesId   INTEGER NOT NULL REFERENCES Games (id),
    regionsId INTEGER NOT NULL REFERENCES Regions (id),
    CONSTRAINT uniqueGamesRegionsId UNIQUE (gamesId, regionsId)
);

CREATE TABLE Tags
(
    id   INTEGER PRIMARY KEY,
    name VARCHAR(255) NOT NULL UNIQUE
);

CREATE TABLE GamesTags
(
    gamesId INTEGER NOT NULL REFERENCES Games (id),
    tagsId  INTEGER NOT NULL REFERENCES Tags (id),
    CONSTRAINT uniqueGamesTagsId UNIQUE (gamesId, tagsId)
);

CREATE TABLE Catalogs
(
    id            INTEGER PRIMARY KEY,
    name          VARCHAR(255) NOT NULL UNIQUE,
    uniqueName    VARCHAR(255) NOT NULL UNIQUE,
    url           TEXT         NOT NULL UNIQUE,
    lastUpdateAt  TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    version       VARCHAR(255),
    priority      INTEGER      NOT NULL DEFAULT 0,
    updatePending BOOLEAN               DEFAULT FALSE,
    json          JSONB        NOT NULL,
    latestJson    JSONB
);

CREATE TABLE Systems
(
    id         INTEGER PRIMARY KEY,
    catalogsId INTEGER      NOT NULL REFERENCES Catalogs (id),
    name       VARCHAR(255) NOT NULL,
    uniqueName VARCHAR(255) NOT NULL UNIQUE,
    dbPath     TEXT
);

CREATE TABLE Cores
(
    id         INTEGER PRIMARY KEY,
    catalogsId INTEGER      NOT NULL REFERENCES Catalogs (id),
    name       VARCHAR(255) NOT NULL,
    uniqueName VARCHAR(255) NOT NULL UNIQUE,
    rbfPath    TEXT
);

CREATE TABLE CoresTags
(
    id      INTEGER PRIMARY KEY,
    coresId INTEGER NOT NULL REFERENCES Cores (id),
    tagsId  INTEGER NOT NULL REFERENCES Tags (id),
    CONSTRAINT coresTagsUnique UNIQUE (coresId, tagsId)
);

CREATE TABLE CoresSystems
(
    id        INTEGER PRIMARY KEY,
    coresId   INTEGER NOT NULL REFERENCES Cores (id),
    systemsId INTEGER NOT NULL REFERENCES Systems (id),
    CONSTRAINT coresSystemsUnique UNIQUE (coresId, systemsId)
);

CREATE TABLE Shortcuts
(
    id       INTEGER PRIMARY KEY,
    usersId  INTEGER      NOT NULL REFERENCES Users (id),
    key      VARCHAR(255) NOT NULL,
    shortcut TEXT         NOT NULL,
    meta     JSON,
    CONSTRAINT shortcutsUsersIdKey UNIQUE (usersId, shortcut)
);

CREATE TABLE Screenshots
(
    id        INTEGER PRIMARY KEY,
    gamesId   INTEGER NOT NULL REFERENCES Games (id),
    path      TEXT    NOT NULL,
    usersId   INTEGER NOT NULL REFERENCES Users (id),
    createdAt TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE VIEW ExtendedGamesView AS
SELECT Games.id                                        AS id,
       Systems.name                                    AS systemName,
       IFNULL(UserGames.coresId, CoresSystems.coresId) AS coresId,
       Games.path                                      AS romPath,
       Cores.rbfPath                                   AS rbfPath,
       Games.name                                      AS name,
       UserGames.favorite                              AS favorite,
       UserGames.lastPlayedAt                          AS lastPlayedAt
FROM Games
         LEFT JOIN CoresSystems ON CoresSystems.systemsId = Games.systemsId OR CoresSystems.coresId = Games.coresId
         LEFT JOIN Systems ON Games.systemsId = Systems.id OR CoresSystems.systemsId = Systems.id
         LEFT JOIN Cores ON CoresSystems.coresId = Cores.id
         LEFT JOIN UserGames ON UserGames.gamesId = Games.id
;
`,
  // -- v2: playlists (old frontend 2025-05-20) --
  `
CREATE TABLE Playlists
(
    id       INTEGER PRIMARY KEY,
    name     TEXT    NOT NULL,
    usersId  INTEGER NOT NULL REFERENCES Users (id),
    isPublic BOOLEAN DEFAULT FALSE,
    CONSTRAINT PlaylistsNameUsersIdUnique UNIQUE (name, usersId)
);

CREATE TABLE PlaylistsGames
(
    playlistsId INTEGER REFERENCES Playlists (id) ON DELETE CASCADE,
    gamesId     INTEGER REFERENCES Games (id) ON DELETE CASCADE,
    priority    INTEGER,
    CONSTRAINT PlaylistsGamesPlaylistsIdGamesIdUnique UNIQUE (playlistsId, gamesId)
);
`,
  // -- v3: play sessions (old frontend 2025-07-30) --
  `
CREATE TABLE Sessions
(
    id            INTEGER PRIMARY KEY,
    usersId       INTEGER   NOT NULL REFERENCES Users (id),
    gamesId       INTEGER REFERENCES Games (id) ON DELETE CASCADE,
    startedAt     TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    secondsPlayed INTEGER            DEFAULT 1,
    CONSTRAINT sessionUsersGamesStartedAt UNIQUE (usersId, gamesId, startedAt)
);
`,
];

export const SCHEMA_VERSION = MIGRATIONS.length;
