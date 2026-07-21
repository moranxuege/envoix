#!/usr/bin/env python3
"""Apply the two reviewed Swift-only fixes to a UniFFI binding."""

from pathlib import Path
import sys

EXPECTED_CALLBACK_VTABLES = 5


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count == 1:
        return text.replace(old, new)
    if count == 0 and text.count(new) == 1:
        return text
    raise SystemExit(f"error: expected one raw or reviewed {label} binding pattern")


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: postprocess-apple-binding.py <envoix_ffi.swift>")
    path = Path(sys.argv[1])
    text = path.read_text()
    text = replace_once(
        text,
        """        return String(bytes: bytes, encoding: String.Encoding.utf8)!""",
        """        // Use Swift's native UTF-8 decoder; `String(bytes:encoding:.utf8)` goes
        // through Foundation's NSString and silently strips a leading U+FEFF BOM.
        // Invalid UTF-8 substitutes U+FFFD instead of trapping (unreachable
        // given Rust's `String` invariant).
        return String(decoding: bytes, as: UTF8.self)""",
        "UTF-8 lift",
    )
    text = replace_once(
        text,
        """        return String(bytes: try readBytes(&buf, count: Int(len)), encoding: String.Encoding.utf8)!""",
        """        // See `lift` above for why we avoid Foundation's NSString-backed decoder here.
        return String(decoding: try readBytes(&buf, count: Int(len)), as: UTF8.self)""",
        "UTF-8 read",
    )
    old_vtable = """    static let vtablePtr: UnsafePointer<"""
    new_vtable = """    //
    // `nonisolated(unsafe)` is needed under Swift 6 strict concurrency.
    // This is safe because the pointee is initialized once during static init
    // and never mutated by either side of the FFI.  Its fields are C function pointers.
    nonisolated(unsafe) static let vtablePtr: UnsafePointer<"""
    raw_count = text.count(old_vtable)
    reviewed_count = text.count(new_vtable)
    if raw_count + reviewed_count != EXPECTED_CALLBACK_VTABLES:
        raise SystemExit(
            "error: expected "
            f"{EXPECTED_CALLBACK_VTABLES} callback vtables, found "
            f"{raw_count + reviewed_count}"
        )
    text = text.replace(old_vtable, new_vtable)
    text = "\n".join(line.rstrip() for line in text.splitlines()) + "\n"
    path.write_text(text)


if __name__ == "__main__":
    main()
