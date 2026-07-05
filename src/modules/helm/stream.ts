// Helmsmen — pure stream buffer for Agent Session output (task #6).
//
// PTY output is hostile on every Runtime. This buffer treats it strictly
// as data: bytes are decoded to text and accumulated, never parsed,
// never interpreted, never turned into markup. Rendering stays safe as
// long as consumers only ever assign the snapshot to `textContent`
// (guard-tested in streamView.test.ts).

export interface StreamBuffer {
  /** Append a chunk; returns the full retained text snapshot. */
  append(chunk: Uint8Array | string): string;
  text(): string;
}

const DEFAULT_LIMIT = 200_000;

export function createStreamBuffer(limit = DEFAULT_LIMIT): StreamBuffer {
  // stream:true carries multi-byte UTF-8 sequences split across chunks.
  const decoder = new TextDecoder("utf-8", { fatal: false });
  let text = "";

  return {
    append(chunk) {
      text +=
        typeof chunk === "string"
          ? chunk
          : decoder.decode(chunk, { stream: true });
      if (text.length > limit) {
        // Backfill buffer only: trimming the front may slice an escape
        // sequence, which is harmless data in a textContent sink.
        text = text.slice(text.length - limit);
      }
      return text;
    },
    text: () => text,
  };
}
