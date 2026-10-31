# Security model

`ctypst` provides a narrow, deterministic I/O boundary around embedded Typst.
It is designed for applications that own and review their Typst templates while
supplying user data as inputs or virtual files.

The default engine has no filesystem root, registered packages, system fonts,
network client, shell, or Typst executable. If a caller explicitly adds a root,
imports are confined to its canonical directory and link escapes are rejected.
All reads are byte-limited while they happen. Virtual-file changes made by a
compile request are applied under the compilation lock and rolled back after
success or failure. Fonts, inputs, files, paths, pages, PDFs, and raster output
have finite configurable limits.

Typst packages are never downloaded. An application can register the files of
an exact `@namespace/name:version` package; those files are validated against
the package manifest and path and size limits at build time and are then
served from memory. Registered package files are trusted application code,
like templates: review them as such. Imports of any other package or version
fail, and package paths cannot reach the configured root or virtual files.

This is a capability boundary, not an execution sandbox. Typst compilation
runs in the caller's process, so a hostile template could still consume CPU or
memory before a post-compilation limit can reject its output. Do not compile
untrusted template code in a privileged or long-lived process. Put it in a
separate worker with OS-enforced memory, CPU, wall-clock, process, and filesystem
limits. Treat the configured root as non-adversarial during a compilation;
canonical path checks do not promise protection against concurrent filesystem
replacement attacks.

Report suspected vulnerabilities privately to the repository owner through
GitHub's private vulnerability reporting. Do not include personal CV data or
other secrets in a public issue.
