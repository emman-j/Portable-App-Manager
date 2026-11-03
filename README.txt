APP ICON
========
Replace  icon.png  in this folder with your own icon and rebuild (cargo build --release).

- Any common image format works (PNG recommended), ideally square, 256x256 or larger.
- It is used for the window / taskbar icon (loaded at runtime from the embedded PNG)
  and, via build.rs, converted to a multi-size .ico and embedded in the Windows .exe.
- Keep the file name `icon.png`.
