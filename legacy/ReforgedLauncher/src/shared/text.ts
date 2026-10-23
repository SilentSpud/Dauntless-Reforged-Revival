// Characters that must never reach the UI from the network: C0 and C1 control characters, and the
// bidirectional overrides and isolates that can make one text look like another. The expressions
// are built from code points so that this source file stays plain ASCII.

const cp = (n: number): string => String.fromCodePoint(n);
const BIDI = `${cp(0x202a)}-${cp(0x202e)}${cp(0x2066)}-${cp(0x2069)}`;

// Any control character (line breaks included) or bidi control.
export const UNSAFE_CHAR = new RegExp(`[\\x00-\\x1f\\x7f-\\x9f${BIDI}]`);
export const UNSAFE_CHARS = new RegExp(`[\\x00-\\x1f\\x7f-\\x9f${BIDI}]`, "g");

// The same, but line feeds are allowed (multi-line news bodies).
export const UNSAFE_CHARS_MULTILINE = new RegExp(`[\\x00-\\x09\\x0b-\\x1f\\x7f-\\x9f${BIDI}]`, "g");

// For tests and fixtures: a right-to-left override and a bell character.
export const RLO = cp(0x202e);
export const BELL = cp(0x07);
