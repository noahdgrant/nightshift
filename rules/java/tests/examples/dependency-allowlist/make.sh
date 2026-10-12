#!/bin/sh
# Writes pass-jdk-all.java into $1: one `import <package>.*;` for each package the running JDK's
# java.* modules export, so dropping one from utils/jdk-package.yml, or a JDK that adds one,
# gives a finding. The file is only scanned, never compiled.
set -eu
for module in $(java --list-modules | sed -n 's/^\(java\.[^@]*\)@.*/\1/p'); do
  java --describe-module "$module" | awk '$1 == "exports" && NF == 2 { print "import " $2 ".*;" }'
done | sort -u >"$1/pass-jdk-all.java"
echo 'class AllJdk {}' >>"$1/pass-jdk-all.java"
