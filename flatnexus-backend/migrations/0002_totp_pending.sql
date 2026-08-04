-- Stage a re-enrolled TOTP secret instead of replacing the live one.
--
-- Enrollment previously wrote straight into `totp_secret` and cleared
-- `totp_confirmed`, while leaving `otp_method` at 'totp'. That put an account
-- with working two-factor into the "TOTP selected but never confirmed" state,
-- which `login` deliberately treats as having no usable second factor and
-- completes on the password alone. Merely *opening* re-enrollment and walking
-- away therefore downgraded a protected account to password-only sign-in.
--
-- With a pending column, the confirmed secret stays authoritative until a code
-- from the new device proves the new one works.

BEGIN;

ALTER TABLE publishers
    ADD COLUMN IF NOT EXISTS totp_pending_secret TEXT;

COMMIT;
