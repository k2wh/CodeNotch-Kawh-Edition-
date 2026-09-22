#!/usr/bin/env python3
"""Generate the SQLite fixtures used by codenotch-core's reader tests.

These are produced by real SQLite so the hand-written reader in
`core/src/sqlite.rs` is checked against the actual on-disk format rather than
against our own assumptions about it.

Usage: python3 scripts/gen_test_fixtures.py
"""

import json
import os
import shutil
import sqlite3

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "src-tauri", "core", "tests", "fixtures")


def reset(path):
    for suffix in ("", "-wal", "-shm", "-journal"):
        p = path + suffix
        if os.path.exists(p):
            os.remove(p)


def make_simple(path):
    """A rollback-journal database with overflow payloads and a 3-level b-tree."""
    reset(path)
    con = sqlite3.connect(path)
    # A tiny page size forces both overflow pages and interior b-tree pages,
    # which is exactly the machinery we want covered.
    con.execute("PRAGMA page_size=512")
    con.execute("PRAGMA journal_mode=delete")
    con.execute("VACUUM")
    con.execute("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)")
    rows = [
        ("cursorAuth/stripeMembershipType", b"pro"),
        ("cursorAuth/cachedEmail", b"dev@example.com"),
        # Comfortably past a 512-byte page: exercises the overflow chain.
        ("bigValue", ("x" * 9000).encode()),
        ("unicode/éè", "café".encode()),
        ("emptyValue", b""),
    ]
    # Enough filler rows to push the table b-tree past a single leaf page.
    rows += [(f"filler/{i:04d}", f"value-{i}".encode()) for i in range(400)]
    con.executemany("INSERT INTO ItemTable VALUES (?, ?)", rows)
    con.commit()
    con.close()
    print("wrote", path, os.path.getsize(path), "bytes")


def make_wal(path):
    """A WAL database whose newest rows are NOT checkpointed into the main file.

    We snapshot the files while the connection is still open, because closing it
    would checkpoint and delete the WAL -- and the uncheckpointed case is the
    one the reader has to get right for a running Cursor.
    """
    live = path + ".live"
    reset(live)
    reset(path)
    con = sqlite3.connect(live)
    con.execute("PRAGMA page_size=512")
    con.execute("PRAGMA journal_mode=wal")
    con.execute("PRAGMA wal_autocheckpoint=0")
    con.execute("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)")
    con.execute("INSERT INTO ItemTable VALUES (?, ?)", ("checkpointed", b"in-main-file"))
    con.commit()
    con.execute("PRAGMA wal_checkpoint(FULL)")

    # Everything below lands in the WAL only.
    con.execute("INSERT INTO ItemTable VALUES (?, ?)", ("onlyInWal", b"from-the-wal"))
    con.execute(
        "INSERT INTO ItemTable VALUES (?, ?)",
        ("walOverflow", ("w" * 9000).encode()),
    )
    con.commit()
    # An updated value must read back as the WAL version, not the stale page.
    con.execute("UPDATE ItemTable SET value=? WHERE key=?", (b"updated-in-wal", "checkpointed"))
    con.commit()

    # An uncommitted transaction: its frames must be ignored by the reader.
    con.execute("BEGIN")
    con.execute("INSERT INTO ItemTable VALUES (?, ?)", ("uncommitted", b"must-not-appear"))

    shutil.copyfile(live, path)
    shutil.copyfile(live + "-wal", path + "-wal")
    con.rollback()
    con.close()
    reset(live)
    print("wrote", path, os.path.getsize(path), "bytes +",
          os.path.getsize(path + "-wal"), "bytes of WAL")


def make_composers(path):
    """A workspace database holding Cursor composer sessions.

    Timestamps are anchored to 2026-01-01T12:00:00Z (epoch 1767268800) so the
    adapter tests can assert exact activity states against a fixed "now".
    """
    reset(path)
    now_ms = 1767268800 * 1000
    con = sqlite3.connect(path)
    con.execute("PRAGMA journal_mode=delete")
    con.execute("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)")
    composer_data = json.dumps({
        "allComposers": [
            # 5s old -> generating
            {"composerId": "c1", "name": "Refactor auth", "lastUpdatedAt": now_ms - 5_000},
            # 3min old -> done
            {"composerId": "c2", "name": "Fix flaky test", "lastUpdatedAt": now_ms - 180_000},
            # Days old -> filtered out of the session list entirely
            {"composerId": "c3", "name": "Ancient", "lastUpdatedAt": now_ms - 400_000_000},
        ]
    })
    con.execute(
        "INSERT INTO ItemTable VALUES (?, ?)",
        ("composer.composerData", composer_data.encode()),
    )
    con.commit()
    con.close()
    print("wrote", path, os.path.getsize(path), "bytes")


def main():
    os.makedirs(OUT, exist_ok=True)
    make_simple(os.path.join(OUT, "simple.db"))
    make_wal(os.path.join(OUT, "wal.db"))
    make_composers(os.path.join(OUT, "composers.db"))


if __name__ == "__main__":
    main()
