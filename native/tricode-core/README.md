# tricode-core

Encodes text into codes made of triangles and reads them back from images.
Codes carry Reed-Solomon error correction, so they still decode with a logo or
text in the centre, mirrored, or with inverted colours.

`generate_image_debug` renders a code to PNG bytes and `decode_image` reads one
from PNG bytes. For use from the terminal, see
[`tricode`](https://crates.io/crates/tricode).

## License

MIT
