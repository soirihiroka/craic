The macOS app icon is an Apple Icon Composer document in `CraicIcon.icon`.
`Assets/GlassyMark.png` preserves the preferred amber glass connection mark and
three pearl nodes on a clean dark graphite background. The artwork remains dark
in both Default and Dark appearances. Icon Composer supplies the native outer
shape and compatibility renditions. Additional glass effects on the artwork
layer are disabled to preserve the mark's existing shading.

Run `make macos-icon` with Xcode's Icon Composer toolchain (verified with 27.2) to compile the native `Assets.car`
and compatibility `CraicIcon.icns`. `make macos-app` bundles both and declares
`CFBundleIconName` so macOS can render the native layered icon. Older macOS
versions use the compiler's compatibility icon.

`CraicIcon.png` is the 1024-pixel default-appearance preview exported by Apple's
`ictool` from this document, using the macOS 27 design generation. It is not the
app's native icon source.

Artwork provenance: built-in imagegen tool, edit mode, opaque output. The edit
used the previously generated amber-glass Craic icon as its reference, preserving
the preferred mark while replacing its reflective tile background. The selected
result is stored in the project as `CraicIcon.icon/Assets/GlassyMark.png`.

Exact final edit prompt:

> Use case: precise-object-edit. Asset type: artwork layer inside a native macOS app icon. Keep the existing amber glass C connection mark and three pearl nodes unchanged: same beautiful amber refraction, clear glass highlights, shape, proportions and location. Change ONLY the backdrop. Replace the entire rounded-square tile, its rim, reflections and all outer canvas with a perfectly smooth continuous uniform dark graphite color #292D35 filling the complete square canvas edge to edge. The C and pearl nodes should float on this simple smooth dark field. The background is untextured flat dark graphite, quiet and clean, with no additional objects or surface detail. There must be NO rounded-square boundary or border in this image: Apple will apply its native shape afterward. Preserve every highlight and detail on the amber C and pearl nodes themselves, but remove the old tile reflections completely. Output an opaque full square composition with the same canvas and unchanged mark placement. Do not add words.
