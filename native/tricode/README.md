# tricode

Writes codes made of triangles as PNGs and reads them back from images. Codes
carry Reed-Solomon error correction, so they still decode with a logo in the
centre, mirrored, or with inverted colours.

```shell
cargo install tricode

tricode encode "hi" -o hi.png
tricode decode hi.png
```

`decode` reads PNG, JPEG and WebP, and prints the text to stdout. Run
`tricode encode --help` for styles, colours, error correction levels and centre
overlays.

## License

MIT
