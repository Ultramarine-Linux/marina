# hacking on marina

you may run:

```bash
cargo run -p marina-ui-slint --features debug-mcp,live-preview
```

to get a debug build of marina-ui-slint with live preview and MCP enabled,
refer to <.zed/debug.json> for the debug environment setup

this allows for you to live-edit the UI and see changes in real-time,
and also allow for clankers to test the UI if you're
into that kind of vibe testing that shit

## getting started

to get started, get build dependencies for slint (`fontconfig-devel`), then configure a local SQLite database and/or use your existing RomM library for fixtures

### prerequisites

- fontconfig-devel
- clang-devel
- lld
- 

```env
ROMM_TOKEN=my_romm_token
# cappy's personal RomM server, so you can fetch assets from the fixtures below
ROMM_URL=https://romm.cappuchino.xyz
# Root directory containing locally installed games. The local library is
# intentionally based on installed content; RomM remains the online Store and
# save-sync source.
MARINA_LIBRARY_ROOT=/path/to/marina/library
# SQLite database path; the default is sqlite://marina.db
MARINA_STORAGE_URI=sqlite://marina.db

#
#ROMM_LIMIT=1
# test search
MARINA_SEARCH=mario

# uncomment below to enable import from your RomM library for fixtures
# use the marina-test crate to import those fixtures
# ROMM_IMPORT=true
```

you may also want to use cappy's library dump at <test/fixtures/cappy-romm.surql.zst>

it has a lot of games (~6000) and is the test for a big game database from RomM
