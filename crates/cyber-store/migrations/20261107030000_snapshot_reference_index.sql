-- A derived, event-owned index. Keep bookkeeping in the event transaction even
-- for imports or metadata repairs that do not use the runtime's projectors.
CREATE TABLE snapshot_reference (
    event_id TEXT NOT NULL REFERENCES event(id) ON DELETE CASCADE,
    root TEXT NOT NULL CHECK (
        length(root)=69 AND substr(root,1,5)='copy:'
        AND substr(root,6) NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY(event_id,root)
) STRICT;
CREATE INDEX snapshot_reference_root ON snapshot_reference(root);
INSERT INTO snapshot_reference(event_id,root)
SELECT DISTINCT e.id,j.atom FROM event e,json_tree(e.data) j
WHERE j.type='text' AND length(j.atom)=69 AND substr(j.atom,1,5)='copy:'
AND substr(j.atom,6) NOT GLOB '*[^0-9a-f]*';
CREATE TRIGGER snapshot_reference_insert AFTER INSERT ON event BEGIN
    INSERT OR IGNORE INTO snapshot_reference(event_id,root)
    SELECT NEW.id,j.atom FROM json_tree(NEW.data) j
    WHERE j.type='text' AND length(j.atom)=69 AND substr(j.atom,1,5)='copy:'
    AND substr(j.atom,6) NOT GLOB '*[^0-9a-f]*';
END;
CREATE TRIGGER snapshot_reference_update AFTER UPDATE OF data ON event BEGIN
    DELETE FROM snapshot_reference WHERE event_id=NEW.id;
    INSERT OR IGNORE INTO snapshot_reference(event_id,root)
    SELECT NEW.id,j.atom FROM json_tree(NEW.data) j
    WHERE j.type='text' AND length(j.atom)=69 AND substr(j.atom,1,5)='copy:'
    AND substr(j.atom,6) NOT GLOB '*[^0-9a-f]*';
END;
