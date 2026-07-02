-- QR-code uniqueness must ignore soft-deleted teams so a deleted team's QR code
-- can be reused (CRD 1844, revised). The original partial unique index only
-- excluded NULL qr_code; recreate it to also exclude soft-deleted rows.
DROP INDEX IF EXISTS idx_teams_qr_code;
CREATE UNIQUE INDEX idx_teams_qr_code ON teams(qr_code)
    WHERE qr_code IS NOT NULL AND deleted_at IS NULL;
