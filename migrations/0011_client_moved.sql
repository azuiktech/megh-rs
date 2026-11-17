-- Client identity (docs/TPD-messaging.md): a guest whose chat moved to the client it turned out to be is soft-deleted
-- with that client's id, so the browser that was the guest is answered as the client.
ALTER TABLE clients ADD COLUMN IF NOT EXISTS moved_to UUID;
