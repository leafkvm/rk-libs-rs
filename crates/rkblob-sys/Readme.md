## rkblob-sys

Compile command:

```
env CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=$HOME/leafkvm/buildroot/output/host/bin/arm-linux-gcc CARGO_BUILD_TARGET=armv7-unknown-linux-gnueabihf PKG_CONFIG_SYSROOT_DIR=$HOME/leafkvm/buildroot/output/staging PKG_CONFIG_PATH=$HOME/leafkvm/buildroot/output/staging/usr/lib/pkgconfig cargo build --target=armv7-unknown-linux-gnueabihf
```