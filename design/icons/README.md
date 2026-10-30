# Icons

`source.mjs` holds the clean geometry of 84 icons on a 24 px grid.
`build.mjs` redraws each icon by hand-like strokes. It makes three versions of each icon (0, 1 and 2). The app plays them in turn at 8 fps for the shimmer.

## Run

From the repo root:

    node design/icons/build.mjs /tmp/iconbuild/node_modules

The argument is a `node_modules` folder that holds `svg-path-properties`. The output is deterministic.

## Output

In `design/icons/`:

- `svg/<name>.svg`: version 0 in `currentColor`.
- `versions/<name>-<v>.svg`: three versions in specimen colours.
- `specimen.html`: a preview page. It uses absolute paths, so run the script on the machine where you open it.

In `crates/consort-desktop/assets/icons/`, all in `currentColor`:

- `<name>.svg`: version 0.
- `v1/<name>.svg`: version 1.
- `v2/<name>.svg`: version 2.

The script overwrites app icons that have the same name.
