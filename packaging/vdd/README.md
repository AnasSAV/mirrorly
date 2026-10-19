# Virtual Display Driver (bundled)

The virtual monitor that an iPad shows when it is used as a display.
Mirrorly installs it the first time **Use as display** is chosen (one
administrator prompt), from this folder next to `Mirrorly.exe`.

| | |
|---|---|
| Project | [VirtualDrivers/Virtual-Display-Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) |
| Release | 24.12.24, `Signed-Driver-v24.12.24-x64.zip` |
| Zip SHA-256 | `f93e7ce3d640c83c419b1b72d2c83b2d8e34d83e8b7ba11f3328a40d2da82fa4` |
| Signed by | SignPath Foundation (catalog `mttvdd.cat`) |
| License | MIT, see `LICENSE.txt` |

The files are the zip's `MttVDD.dll`, `MttVDD.inf` and `mttvdd.cat`,
unmodified. Its sample `vdd_settings.xml` is not used: Mirrorly writes
its own (one monitor, iPad resolutions) to `C:\VirtualDisplayDriver`.
