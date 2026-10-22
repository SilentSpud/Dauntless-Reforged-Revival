// Usernames: 3-16 characters, letters A-Z/a-z, digits and underscore (the same rule as Dauntless
// Revived). Uniqueness (case-insensitive) is the server's job; this is the live check in the form.

export const USERNAME_MIN = 3;
export const USERNAME_MAX = 16;

export type UsernameCheck = "ok" | "empty" | "too_short" | "too_long" | "bad_chars";

export function checkUsername(input: unknown): UsernameCheck {
  if (typeof input !== "string" || input.length === 0) return "empty";
  if (!/^[A-Za-z0-9_]*$/.test(input)) return "bad_chars";
  if (input.length < USERNAME_MIN) return "too_short";
  if (input.length > USERNAME_MAX) return "too_long";
  return "ok";
}

export function isValidUsername(input: unknown): input is string {
  return checkUsername(input) === "ok";
}

// Account keys go on the game's command line (-AUTH_PASSWORD=<key>), so only plain characters
// are accepted. Keys made by a Reforged server look like RRK_<48 hex>.
export function isPlausibleAccountKey(input: unknown): input is string {
  return typeof input === "string" && /^[A-Za-z0-9_-]{8,128}$/.test(input);
}

// A pasted key may come with spaces or line breaks around it, or as the whole backup file.
export function extractAccountKey(text: string): string | null {
  if (typeof text !== "string" || text.length > 8192) return null;
  const labelled = /^\s*Key:\s*([A-Za-z0-9_-]{8,128})\s*$/m.exec(text);
  if (labelled) return labelled[1];
  const trimmed = text.trim();
  if (isPlausibleAccountKey(trimmed)) return trimmed;
  // A backup file pasted into a one-line field loses its line breaks; a server-made key is
  // still recognisable by its exact shape.
  const made = /Key:\s*(RRK_[0-9a-fA-F]{48})(?![0-9a-fA-F])/.exec(text);
  return made ? made[1] : null;
}
