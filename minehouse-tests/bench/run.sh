#!/bin/bash
# One benchmark run against a Steel binary.
#
# Usage: run.sh TAG REGIONIZED(0|1) BOT_SPEED MINUTES BOTS STEEL_BIN SERVER_DIR
#
# SERVER_DIR holds a prepared config/ (offline, no encryption, allow_flight,
# compression threshold 2147483647, [log] extra = true, seed 777, hard, creative,
# minehouse modules disabled); its saves are wiped first. Writes TAG.log (server)
# and TAG.driver.log next to this script, then prints the phase averages.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
TAG=$1; REG=$2; SPEED=$3; MIN=$4; N=$5; BIN=$6; S=$7
rm -rf "$S/saves" "$S/.logs" "$HERE/$TAG.log"
FIFO=$(mktemp -u)
export STEEL_REGIONIZED_TICKING=$REG
python3 "$HERE/ptyrun.py" "$FIFO" "$HERE/$TAG.log" "$S" -- "$BIN" &
PTY=$!
sleep 15
BOT_SPEED=$SPEED python3 "$HERE/bench.py" "$FIFO" 25565 "$N" "$MIN" "$TAG" > "$HERE/$TAG.driver.log" 2>&1
echo stop > "$FIFO"
sleep 20
kill $PTY 2>/dev/null
python3 "$HERE/phases.py" "$HERE/$TAG.log" 60
python3 "$HERE/phases.py" "$HERE/$TAG.log" 60 "Server tick slow"
