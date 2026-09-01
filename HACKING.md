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

to get started, get build dependencies for slint (`fontconfig-devel`), and then spin up a surrealdb database for the fixtures, or use your existing RomM library if you have one

### prerequisites

- fontconfig-devel
- clang-devel
- lld
- 

```env
ROMM_TOKEN=my_romm_token
# cappy's personal RomM server, so you can fetch assets from the fixtures below
ROMM_URL=https://romm.cappuchino.xyz
# for hacking on database, you may use a hosted surrealdb instance
MARINA_STORAGE_URI=ws://localhost:8000
# for prod you will be using local surrealkv
#MARINA_STORAGE_URI=rocksdb://.data/

# surrealdb credentials for testing database, not needed for prod
MARINA_STORAGE_USERNAME=root
MARINA_STORAGE_PASSWORD=root

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
