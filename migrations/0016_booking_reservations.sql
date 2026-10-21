-- Places taken, one row per place: a room, provider or client during a time, by an appointment or a hold
-- (owner). Capacity left is a participant's capacity minus the most rows overlapping at once; `waitlist` rows
-- count against its waitlist instead. A hold's rows expire; cancel and release delete an owner's rows.
CREATE EXTENSION IF NOT EXISTS btree_gist;

CREATE TABLE IF NOT EXISTS reservations (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    owner_id UUID NOT NULL,
    participant_id UUID NOT NULL,
    during TSTZRANGE NOT NULL,
    waitlist BOOLEAN NOT NULL,
    expires_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id)
);
CREATE INDEX IF NOT EXISTS reservations_taken ON reservations USING gist (org_id, participant_id, during);
CREATE INDEX IF NOT EXISTS reservations_owner ON reservations (org_id, owner_id);

DO $$
BEGIN
    IF to_regclass('occupancies') IS NOT NULL THEN
        INSERT INTO reservations (org_id, id, owner_id, participant_id, during, waitlist)
        SELECT o.org_id, gen_random_uuid(), o.appointment_id, o.participant_id, o.during, a.status = 'waitlist'
        FROM occupancies o
        JOIN appointments a ON a.org_id = o.org_id AND a.id = o.appointment_id
        WHERE a.deleted_at IS NULL AND a.status IN ('waitlist', 'booked', 'confirmed');
        DROP TABLE occupancies;
    END IF;
END $$;
