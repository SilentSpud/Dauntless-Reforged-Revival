#!/bin/sh
# Writes rh3.json next to this script with the Node kit's own code (see gen.ts).
#   sh rust/services/host/tests/fixtures/node/rh3/run.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../../../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp "$repo"/legacy/ReforgedHost/src/*.ts "$work"/
cp "$here/gen.ts" "$work"/
# Type stripping needs the imports' extensions; cli.ts runs main() only as a CommonJS program; the
# two constructors with parameter properties are spelled out (strip-only mode refuses them).
for f in "$work"/*.ts; do
    sed -i -E 's#from "\./([a-z]+)"#from "./\1.ts"#; s#^if \(require\.main === module\) \{#if (false) {#' "$f"
done
sed -i 's#^    constructor(readonly code: UpdateProblem, readonly detail: string | null = null) {#    readonly code: UpdateProblem; readonly detail: string | null;\n    constructor(code: UpdateProblem, detail: string | null = null) {#; s#^        super(detail ? `${code}: ${detail}` : code);#        super(detail ? `${code}: ${detail}` : code); this.code = code; this.detail = detail;#' "$work/selfupdate.ts"
sed -i 's#^    constructor(private readonly logDir: string, private readonly print: Printer, private readonly platform: NodeJS.Platform = process.platform) {}#    private readonly logDir: string; private readonly print: Printer; private readonly platform: NodeJS.Platform;\n    constructor(logDir: string, print: Printer, platform: NodeJS.Platform = process.platform) { this.logDir = logDir; this.print = print; this.platform = platform; }#' "$work/services.ts"
node "$work/gen.ts" "$here/rh3.json"
