-- Exports the CoreProtect history for `/co import`. Run with the DuckDB CLI on a copy of the
-- database (never on the file of a running server):
--
--   cp config/CoreProtect/database.db /tmp/coreprotect.db
--   mkdir coreprotect-export && cd coreprotect-export
--   duckdb -readonly /tmp/coreprotect.db < export.sql
--
-- Then put the folder into the data directory of Steel as `coreprotect-export`.

COPY (
    SELECT b.id, b.time, u."user", u.uuid, w.world, b.x, b.y, b.z, t.type, b.blockdata,
           b.action, b.rolled_back
    FROM block b
    JOIN "user" u ON u.id = b."user"
    JOIN world w ON w.id = b.wid
    JOIN type_map t ON t.id = b.type
    WHERE b.x IS NOT NULL AND b.y IS NOT NULL AND b.z IS NOT NULL
    ORDER BY b.id
) TO 'blocks.csv' (HEADER, DELIMITER ',');

COPY (
    SELECT c.id, c.time, u."user", u.uuid, w.world, c.x, c.y, c.z, t.type, c.amount,
           c.action, c.rolled_back
    FROM container c
    JOIN "user" u ON u.id = c."user"
    JOIN world w ON w.id = c.wid
    JOIN type_map t ON t.id = c.type
    WHERE c.x IS NOT NULL AND c.y IS NOT NULL AND c.z IS NOT NULL
    ORDER BY c.id
) TO 'containers.csv' (HEADER, DELIMITER ',');

COPY (SELECT id, data FROM data_map ORDER BY id) TO 'data_map.csv' (HEADER, DELIMITER ',');
