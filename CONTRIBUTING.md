# Contributing

Thanks for helping and taking the time to read this.

## Issues

- Please create issues! These are often the best way to contribute to open source.
- Open an issue if you want to propose a fix/feature or PR.
- If you don’t want to open an issue, that’s fine. Just ship the PR.

## Adding Examples

Examples live in `docs/examples/`.

1. Create a new folder: `docs/examples/<integration-name>/`
   - Use the technology/service name (lowercase, kebab-case). Example: `docs/examples/niri/`.
2. Add a `README.md` in that folder:
   - What it integrates with
   - Dependencies
   - How it reads `status.json` and/or `transcriptions.json`
   - Screenshot or short demo of the example. If uploading a video, use GitHub web editor file upload/drag'n'drop in the `README.md` if possible.
3. Put any configs/scripts/assets in the same folder.
4. Add a short entry under `## Examples` in docs/examples/README.md` that links to your example with a linked demo/screenshot.
