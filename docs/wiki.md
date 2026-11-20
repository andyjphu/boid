# Knowledge-base viewer

Initialize `kb-wiki` after cloning Boid:

```sh
git submodule update --init --recursive
npm ci --prefix kb-wiki
```

Node.js 22+ is required. Copy `config/boid.example.ini` to `~/.config/boid/config.ini` and set `DEFAULT_SYSTEM_PROMPT_ADDR` to a nonempty UTF-8 prompt file and `KB_ADDR` to the Markdown knowledge-base directory. Both values are required. Environment variables override the file. `BOID_CONFIG` selects another configuration file; `KB_WIKI_ADDR` optionally locates an installed renderer outside this checkout.

Boid serves the viewer at `/wiki/` using its existing browser authentication. The full tree, graph, search, Markdown exports and edit endpoints are protected. Editing writes the original file with an atomic replacement and rejects a save if the document changed after it was opened. Derived renders update in a separate Node process; agent requests do not wait for builds. The builder retains one prior generation for in-flight asset reads. Original KB files are not modified by rendering.

New agents use the configured prompt. Existing agents keep their saved prompt until `/reload`, which rereads `DEFAULT_SYSTEM_PROMPT_ADDR`. A missing or empty replacement file leaves the existing prompt unchanged.

Build output and `wiki-builder.log` live under `BOID_STATE_DIR/wiki`. Source data, generated pages, maps and indexes are private; do not commit them to the renderer or Boid repository.
