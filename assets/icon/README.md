# Application Icon

`anyview.svg` is the master application icon: four viewfinder corners framing a mint-cyan rounded diamond on a deep indigo squircle background.

- Designed on a 1024×1024 macOS icon grid (`(100, 100)`, `824×824`, corner radius `185`), pure vector SVG with no external image or font dependencies.
- Platform icon assets are generated from `anyview.svg` by `tools/icon/make-icons.py`:
  - macOS: `apps/macos/Resources/AppIcon.icns`
  - Windows: `apps/windows/Anyview/Assets/Anyview.ico`
  - Linux: `apps/linux/data/icons/hicolor/`

Re-run `python3 -I tools/icon/make-icons.py` after editing `anyview.svg`.
