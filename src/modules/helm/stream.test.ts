import { describe, expect, it } from "vitest";
import { createStreamBuffer } from "./stream";

// Locks the task #6 hostile-output invariant on the frontend side: PTY
// bytes are data. The buffer must carry escape sequences and would-be
// markup through verbatim (never interpret, never transform into HTML),
// so the only rendering path is inert textContent assignment.

describe("createStreamBuffer", () => {
  it("accumulates chunks and returns the full snapshot", () => {
    const buf = createStreamBuffer();
    expect(buf.append("hello ")).toBe("hello ");
    expect(buf.append(new TextEncoder().encode("world"))).toBe("hello world");
    expect(buf.text()).toBe("hello world");
  });

  it("treats hostile escape sequences as inert data (AC: escape handling)", () => {
    const hostile =
      "\x1b]7;file:///etc/passwd\x07" + // OSC 7 cwd relocation (CVE class)
      "\x1b]52;c;aGVsbXNtZW4=\x07" + // OSC 52 clipboard write
      "\x1bc" + // full terminal reset
      "\x1b]0;owned\x07" + // title change
      "plain tail";
    const buf = createStreamBuffer();
    expect(buf.append(hostile)).toBe(hostile);
    expect(buf.text()).toContain("\x1b]7;file:///etc/passwd\x07");
  });

  it("never turns output into markup", () => {
    const buf = createStreamBuffer();
    const markup = '<img src=x onerror=alert(1)><script>evil()</script>';
    expect(buf.append(markup)).toBe(markup);
  });

  it("decodes multi-byte UTF-8 split across chunk boundaries", () => {
    const bytes = new TextEncoder().encode("café ⚓");
    const buf = createStreamBuffer();
    buf.append(bytes.slice(0, 4)); // splits the é sequence
    buf.append(bytes.slice(4));
    expect(buf.text()).toBe("café ⚓");
  });

  it("survives binary junk without throwing", () => {
    const buf = createStreamBuffer();
    expect(() =>
      buf.append(new Uint8Array([0xff, 0xfe, 0x00, 0x9b, 0x1b])),
    ).not.toThrow();
  });

  it("retains only the tail once past the limit", () => {
    const buf = createStreamBuffer(8);
    buf.append("0123456789");
    expect(buf.text()).toBe("23456789");
  });
});
