#!/usr/bin/env bash
#
# Tests for `new_markers` in `lib.sh`.
#
# Each case is a unified diff, as `git diff --unified=0` prints it, and the
# annotation lines `new_markers` must print for it.
#
# Run: bash .config/loop/missions/rustqual/lib-test.sh

set -uo pipefail

# shellcheck source=lib.sh
source "$(dirname "$0")/lib.sh"
failures=0

check() { # name expected, with the diff on stdin
    local actual
    actual=$(new_markers)
    if [[ $actual != "$2" ]]; then
        failures=$((failures + 1))
        printf 'FAIL %s\n  expected: %q\n  actual:   %q\n' "$1" "$2" "$actual"
    fi
}

check 'a new marker' \
    '// qual:allow(complexity, max_function_lines=80) reason: "one long match"' <<'EOF'
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -4,0 +5 @@ impl Parser {
+    // qual:allow(complexity, max_function_lines=80) reason: "one long match"
EOF

# The diff removes one marker line and adds one, so the number of markers in
# the file is unchanged.
check 'a raised limit on an existing marker' \
    '// qual:allow(srp, file_length=2600) reason: "split in T-1hb8d4j"' <<'EOF'
diff --git a/src/provider.rs b/src/provider.rs
index 1111111..2222222 100644
--- a/src/provider.rs
+++ b/src/provider.rs
@@ -1 +1 @@
-// qual:allow(srp, file_length=2400) reason: "split in T-1hb8d4j"
+// qual:allow(srp, file_length=2600) reason: "split in T-1hb8d4j"
EOF

# The guidance for `DRY-001` names this marker.
check 'paired `qual:inverse` annotations' \
    $'// qual:inverse(decode)\n// qual:inverse(encode)' <<'EOF'
diff --git a/src/codec.rs b/src/codec.rs
index 1111111..2222222 100644
--- a/src/codec.rs
+++ b/src/codec.rs
@@ -9,0 +10 @@
+// qual:inverse(decode)
@@ -20,0 +22 @@ pub fn encode(value: &Value) -> Vec<u8> {
+// qual:inverse(encode)
EOF

# The item lands in another file, at another indentation.
check 'a marker that moves with its item' '' <<'EOF'
diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -10,2 +9,0 @@
-// qual:allow(complexity, magic_numbers) reason: "port numbers"
-fn ports() -> [u16; 2] {
diff --git a/src/b.rs b/src/b.rs
index 3333333..4444444 100644
--- a/src/b.rs
+++ b/src/b.rs
@@ -3,0 +4,2 @@ impl Server {
+    // qual:allow(complexity, magic_numbers) reason: "port numbers"
+    fn ports() -> [u16; 2] {
EOF

check 'a removed marker' '' <<'EOF'
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -7,2 +6,0 @@
-// qual:api
-pub fn unused() {}
EOF

# Lines are counted, so one removal doesn't cancel two additions.
check 'a moved marker and a second copy of it' '// qual:api' <<'EOF'
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -7 +6,0 @@
-// qual:api
@@ -12,0 +12 @@
+// qual:api
@@ -30,0 +31 @@
+// qual:api
EOF

check 'a diff without markers' '' <<'EOF'
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -3 +3 @@
-const RETRIES: u32 = 3;
+const RETRIES: u32 = 5;
EOF

if ((failures > 0)); then
    echo "$failures failed."
    exit 1
fi
echo "All passed."
