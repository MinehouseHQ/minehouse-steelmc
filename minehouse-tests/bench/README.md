# Load benchmark: Steel vs vanilla/Fabric

25 offline bots (protocol 777) join, go creative, get spread on a 5×5 grid 600 blocks
apart at y=100, then the driver samples `tick query` and the non-player entity count
every 30 s.

```sh
# Steel (offline mode, proxy off). The console reads from a terminal, so run it in a pty:
python3 ptyrun.py /tmp/steel.fifo steel.log <server dir> -- ./steel
python3 bench.py /tmp/steel.fifo 25565 25 15 steel
grep -E "Average time per tick|Count:" steel.log
```

For vanilla/Fabric, pipe a FIFO into the server's stdin and pass the same FIFO to
`bench.py`. Use the same seed and difficulty on both, `online-mode=false`,
`allow-flight=true`, compression off (Steel: `threshold = 2147483647`).

With `[log] extra = true` Steel's "Game tick slow" warning shows the per-phase split
(`entity_tick`, `tick_chunks`, ...).

Packet ids come from `steel-registry/build_assets/packets.json`. Note that 26.3 kicks
a client that sends two position packets without a `client_tick_end` in between.

## One run with a phase breakdown

`run.sh TAG REGIONIZED BOT_SPEED MINUTES BOTS STEEL_BIN SERVER_DIR` starts the server in a
pty, runs the bots (flying along +X at `BOT_SPEED` blocks per tick, 0 = idle) and prints
`phases.py` averages of the "Game tick slow" (world phases, islands) and "Server tick slow"
(packet drain and other game-loop steps) warnings. `REGIONIZED=1` sets
`STEEL_REGIONIZED_TICKING=1`.
