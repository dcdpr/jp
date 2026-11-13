## Install

On Linux and macOS:

```sh
curl --proto '=https' --tlsv1.2 -LsSf {download}/install.sh | sh
```

On Windows:

```powershell
powershell -ExecutionPolicy Bypass -c "irm {download}/install.ps1 | iex"
```

Both scripts install `jp` {version} into `~/.cargo/bin`, after checking its
archive against `checksums.txt`.
