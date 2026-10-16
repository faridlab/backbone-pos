-- A session closes once: at most one live closing entry per opening entry. The close locks the
-- session and flips it only while it is open; this index is the floor beneath that, so a second
-- closing entry can never be written for the same session. Stored duplicates leave the index out
-- with a notice instead of failing the migration; the close's lock still refuses new ones.
DO $$
BEGIN
    IF to_regclass('pos.pos_closing_entries') IS NOT NULL
       AND to_regclass('pos.uq_pos_closing_entries_opening_entry') IS NULL THEN
        IF EXISTS (
            SELECT 1 FROM pos.pos_closing_entries
            WHERE (metadata->>'deleted_at') IS NULL
            GROUP BY opening_entry_id HAVING count(*) > 1
        ) THEN
            RAISE NOTICE 'uq_pos_closing_entries_opening_entry not added: a session has two live closing entries';
        ELSE
            CREATE UNIQUE INDEX uq_pos_closing_entries_opening_entry
                ON pos.pos_closing_entries (opening_entry_id)
                WHERE (metadata->>'deleted_at') IS NULL;
        END IF;
    END IF;
END $$;
