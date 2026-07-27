# Third-party notices

laplus's window shows a user interface it did not write. About 80% of the
shipped artifact by size is upstream's built web bundle, reused as-is, and the
licence it is offered under requires its notice to travel with it. This file is
that notice. It is shown by the installer and installed alongside the
application, so a copy of laplus always carries it.

## t3code

The user interface — everything the window renders — is `apps/web` from t3code,
built by upstream's own toolchain and embedded unmodified.

```
MIT License

Copyright (c) 2026 T3 Tools Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The web bundle also contains upstream's own vendored dependencies, compiled into
it by their build. Their notices are upstream's to carry and are not restated
here.

## Rust dependencies

The server is statically linked, so its dependencies are in the artifact too —
`tauri`, `axum`, `tokio`, `rusqlite` (which compiles SQLite in), `notify`,
`portable-pty` and their trees. These are overwhelmingly MIT or Apache-2.0, and
SQLite is public domain.

**This section is not yet a complete notice**, and saying so is better than
implying otherwise: no per-crate licence audit has been run. Ticket 24 records
it as the open half of that criterion. What is complete is the section above,
which covers the reused work this project is actually built on.
