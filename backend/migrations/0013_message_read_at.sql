-- Read-receipt watermark for outbound (agent) messages. Facebook "read" events
-- carry no message ids, only a watermark timestamp, so receipts are stamped on
-- the customer's agent messages sent at or before that watermark (CRD §4.2).
ALTER TABLE messages ADD COLUMN read_at TEXT;
