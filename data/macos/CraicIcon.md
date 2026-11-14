The macOS app icon is an Apple Icon Composer document in `CraicIcon.icon`.
Its two flat SVG layers preserve the Craic connection mark: orange connections
and three white node centers. Icon Composer supplies the background, material,
lighting, shadow, mask, and appearance adaptations. The source layers contain
no baked lighting or effects.

Run `make macos-icon` with Xcode's Icon Composer toolchain (verified with 27.2) to compile the native `Assets.car`
and compatibility `CraicIcon.icns`. `make macos-app` bundles both and declares
`CFBundleIconName` so macOS can render the native layered icon. Older macOS
versions use the compiler's compatibility icon.

`CraicIcon.png` is the 1024-pixel default-appearance preview exported by Apple's
`ictool` from this document, using the macOS 27 design generation. It is not the
app's native icon source. There is no generated-image prompt or baked raster
artwork in the layered source.
