-- What a provider or room is (a service step names the provider role it needs), and the time each
-- participant of an appointment is taken: the room from setup to cleanup, each provider for its steps, the client.
ALTER TABLE providers ADD COLUMN IF NOT EXISTS role TEXT;
ALTER TABLE resources ADD COLUMN IF NOT EXISTS role TEXT;

CREATE TABLE IF NOT EXISTS occupancies (
    org_id UUID NOT NULL,
    appointment_id UUID NOT NULL,
    participant_id UUID NOT NULL,
    during TSTZRANGE NOT NULL,
    FOREIGN KEY (org_id, appointment_id) REFERENCES appointments (org_id, id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS occupancies_during ON occupancies USING gist (during);
CREATE INDEX IF NOT EXISTS occupancies_appointment ON occupancies (org_id, appointment_id);
