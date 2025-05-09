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
    userId    INTEGER      NOT NULL REFERENCES Users (id),
    key       VARCHAR(255) NOT NULL,
    value     JSON         NOT NULL,
    updatedAt TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT userStorageUserIdKey UNIQUE (userId, key)
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
    userId       INTEGER   NOT NULL REFERENCES Users (id),
    coresId      INTEGER   NOT NULL REFERENCES Cores (id),
    favorite     BOOLEAN   NOT NULL DEFAULT FALSE,
    lastPlayedAt TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- The list of all games available on the filesystem, or cores that should
-- be shown in the game list.
CREATE TABLE Games
(
    id        INTEGER PRIMARY KEY,
    -- The given name, either from identification or user renaming.
    name      TEXT,
    -- If the game has a core, it is not a ROM.
    coresId   INTEGER REFERENCES Cores (id),
    -- The game may be from a system. Non-ROM games may not be part of a system
    -- (they are, but through their Cores row).
    systemsId INTEGER REFERENCES Systems (id),
    -- If the game has a path, it is a ROM and not a simple core.
    -- It must also have a size and sha256.
    path      TEXT,
    size      INTEGER,
    sha256    VARCHAR(64), -- Store the SHA in hexa.
    -- If the game is a core, it must have a name.
    CONSTRAINT coreHasName
        CHECK ( (coresId IS NOT NULL AND name IS NOT NULL)
            OR (coresId IS NULL) ),
    -- If the path is specified, size and sha256 must be specified as well.
    CONSTRAINT pathSizeSha256
        CHECK ( (path IS NOT NULL AND size IS NOT NULL AND sha256 IS NOT NULL)
            OR (path IS NULL AND size IS NULL AND sha256 IS NULL) )
);

-- Record information about the games that a user has played.
-- This is not the database of all games available on the system,
-- but rather the games that a user has played.
CREATE TABLE UserGames
(
    id           INTEGER PRIMARY KEY,
    usersId      INTEGER NOT NULL REFERENCES Users (id),
    gamesId      INTEGER NOT NULL REFERENCES Games (id),
    -- If the user selected a core for it. Otherwise, the core will be the
    -- resolved default.
    coresId      INTEGER REFERENCES Cores (id),

    favorite     BOOLEAN NOT NULL DEFAULT FALSE,
    lastPlayedAt TIMESTAMP        DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT userGamesUserIdGamesId UNIQUE (usersId, gamesId)
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
    -- The last time this was checked for an update.
    lastUpdateAt  TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    -- The `version` field.
    version       VARCHAR(255),
    priority      INTEGER      NOT NULL DEFAULT 0,
    updatePending BOOLEAN               DEFAULT FALSE,
    json          JSONB        NOT NULL
);

CREATE TABLE Systems
(
    id         INTEGER PRIMARY KEY,
    catalogsId INTEGER      NOT NULL REFERENCES Catalogs (id),
    name       VARCHAR(255) NOT NULL,
    uniqueName VARCHAR(255) NOT NULL UNIQUE,
    dbPath     TEXT
);

-- Installed cores.
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

-- Shortcut tables. Related to a user and can contain additional
-- free-form information.
CREATE TABLE Shortcuts
(
    id       INTEGER PRIMARY KEY,
    userId   INTEGER      NOT NULL REFERENCES Users (id),

    -- The shortcut unique key, like "resetCore".
    key      VARCHAR(255) NOT NULL,

    -- The shortcut chosen by the user, like "Ctrl+Shift+R".
    shortcut TEXT         NOT NULL,

    -- Any additional metadata.
    meta     JSON,

    -- It is illegal to have two identical shortcuts for the same user.
    -- The system would not know which one to trigger.
    CONSTRAINT shortcutsUserIdKey UNIQUE (userId, shortcut)
);

CREATE TABLE Screenshots
(
    id        INTEGER PRIMARY KEY,
    gamesId   INTEGER NOT NULL REFERENCES Games (id),
    path      TEXT    NOT NULL,
    userId    INTEGER NOT NULL REFERENCES Users (id),
    createdAt TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Views to facilitate selection and listing of games.

-- An extended view which links games and their cores and systems.
CREATE VIEW ExtendedGamesView AS
SELECT Games.id                                        AS id,
       IFNULL(Systems_2.name, Systems.name)            AS systemName,
       IFNULL(UserGames.coresId, CoresSystems.coresId) AS coresId,
       Games.path                                      AS romPath,
       IFNULL(Cores_2.rbfPath, Cores.rbfPath)          AS rbfPath,
       Games.name                                      AS name,
       UserGames.favorite                              AS favorite,
       UserGames.lastPlayedAt                          AS lastPlayedAt
FROM Games
         LEFT JOIN Systems ON Games.systemsId = Systems.id
         LEFT JOIN Cores AS Cores_2 ON Games.coresId = Cores_2.id
         LEFT JOIN CoresSystems ON CoresSystems.id IN (Cores_2.id, Games.coresId, Systems.id)
         LEFT JOIN Systems as Systems_2 ON CoresSystems.systemsId = Systems_2.id
         LEFT JOIN Cores ON Games.coresId = Cores.id OR CoresSystems.coresId = Cores.id
         LEFT JOIN UserGames ON UserGames.gamesId = Games.id
;
