"""Step 4 of the Postgres port: wrap each sqlx query's SQL in `db::sql(...)`.

    python scripts/pg_wrap_sql.py desktop/crates/server/src/teams.rs

**Temporary.** This exists only while `desktop/crates/server` is converted from
`SqlitePool` to `sqlx::Any`, one domain at a time — see plan.md, "Postgres is
unsupported". Delete it when the last domain lands.

Paren-matching rather than regex, because the arguments are multi-line string
literals with backslash continuations and embedded quotes: a regex that picks
the wrong closing paren produces code that still compiles and sends different
SQL. It does leave a dangling comma on multi-line calls
(`"...",\n    , state.backend)`), which is a compile error rather than a silent
one; fix with:

    perl -0pi -e 's/,(\\s*), state\\.backend\\)/$1, state.backend)/g' <file>

Run it once per file. It is not idempotent — a second pass wraps the wrapper.
"""

import sys

CALLS = ("sqlx::query_scalar", "sqlx::query_as", "sqlx::query")
BACKSLASH = chr(92)
QUOTE = chr(34)


def find_open_paren(s, i):
    j = i
    if s.startswith("::<", j):  # turbofish, e.g. query_scalar::<_, i64>
        depth = 0
        while j < len(s):
            if s[j] == "<":
                depth += 1
            elif s[j] == ">":
                depth -= 1
                if depth == 0:
                    j += 1
                    break
            j += 1
    while j < len(s) and s[j] in " \n\t":
        j += 1
    return j if j < len(s) and s[j] == "(" else None


def match_paren(s, open_i):
    depth, k, in_str = 0, open_i, False
    while k < len(s):
        c = s[k]
        if in_str:
            if c == BACKSLASH:
                k += 2
                continue
            if c == QUOTE:
                in_str = False
        else:
            if c == QUOTE:
                in_str = True
            elif c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    return k
        k += 1
    return None


def convert(path):
    s = open(path, encoding="utf-8").read()
    out, i, n = [], 0, 0
    while True:
        hit = None
        for name in CALLS:
            j = s.find(name, i)
            if j != -1 and (hit is None or j < hit[0]):
                hit = (j, name)
        if hit is None:
            out.append(s[i:])
            break
        j, name = hit
        op = find_open_paren(s, j + len(name))
        cl = match_paren(s, op) if op is not None else None
        if op is None or cl is None:
            out.append(s[i : j + len(name)])
            i = j + len(name)
            continue
        out.append(s[i : op + 1])
        out.append("&db::sql(" + s[op + 1 : cl] + ", state.backend)")
        i = cl
        n += 1
    open(path, "w", encoding="utf-8").write("".join(out))
    return n


if __name__ == "__main__":
    for path in sys.argv[1:]:
        print(path, "->", convert(path), "call sites wrapped")
