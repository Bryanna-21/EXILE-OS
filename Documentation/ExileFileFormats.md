# Exile OS File Formats and MIME Policy

Status: Initial repository-backed specification
Scope: File identification, associations, compatibility, and future native formats

## 1. Purpose

Exile OS should provide a coherent file ecosystem without sacrificing
interoperability. Existing formats remain usable wherever Exile has the
necessary application or system support. New Exile-native formats must solve
a defined problem rather than merely rename an existing format.

This document distinguishes four separate capabilities:

- Recognition: identifying a file by its name, extension, or contents.
- Association: identifying an application or icon associated with a file.
- Support: having an application or service capable of reading or writing it.
- Trust: determining whether opening, installing, or executing it is safe.

None of these capabilities automatically implies the others.

## 2. Current repository implementation

The principal MIME registry is:

- `Userland/Libraries/LibCore/MimeData.cpp`

The current icon and extension association configuration is:

- `Base/etc/FileIconProvider.ini`

The registry contains filename-based MIME guesses and byte-signature
recognition. The icon configuration maps patterns to existing icon categories.
These are separate mechanisms and may cover different file types.

The registry is not a complete inventory of every format that Exile can open.
An entry proves that the registry knows how to identify a format in at least
one way; it does not prove that a viewer, editor, codec, or installer exists.

## 3. Existing format families

The following are examples found in the current MIME registry. This list is
an inventory of recognition entries, not a promise that every format can be
opened or edited by an installed application.

| Family | Existing examples |
|---|---|
| Documents and text | PDF, RTF, Markdown, HTML, XHTML, XML, CSV, plain text |
| Programming and web | C/C++ source, JavaScript, CSS, shell scripts, WebAssembly |
| Structured data | JSON, SQLite databases |
| Images | PNG, JPEG, GIF, BMP, TIFF, WebP, SVG, JPEG 2000, QOI, TinyVG |
| Audio | WAV, MP3, FLAC, MIDI, QOA |
| Video and containers | Matroska, WebM |
| Archives and compression | ZIP, 7-Zip, gzip, bzip2, tar |
| Fonts | OTF, TTF, WOFF, WOFF2 |
| System and specialist data | ELF, EXT filesystem signatures, ISO images, QCOW images, DICOM |
| Other recognized data | Blender projects, NES ROMs, Lua bytecode, compressed Windows files |

The existing icon configuration additionally associates patterns for formats
such as `.presenter`, `.sheets`, `.palette`, `.pp`, `.cal`, `.gmi`, `.avi`,
`.mov`, and `.mp4`. These associations do not establish that every listed
format has corresponding MIME registration or application support.

Some existing entries are project-specific or use nonstandard MIME names,
including entries under `extra/`. Such names must not be represented as
officially registered Internet media types without separate verification.

## 4. Compatibility policy

### 4.1 Keep established extensions

Use existing, widely recognized formats for general-purpose content whenever
they meet the product requirements.

Examples include:

- Documents: `.pdf`, `.rtf`, `.txt`, `.md`
- Images: `.png`, `.jpg`, `.jpeg`, `.svg`, `.webp`
- Audio: `.wav`, `.mp3`, `.flac`
- Video: `.mkv`, `.webm`, `.mp4`
- Data: `.json`, `.csv`, `.sqlite`
- Archives: `.zip`, `.7z`, `.gz`, `.tar`

These examples do not imply that every corresponding codec or application is
currently implemented in Exile OS.

### 4.2 Do not create extension aliases without a reason

A new extension should not be introduced solely to make an ordinary file look
Exile-branded. Renaming a PDF, PNG, ZIP, or JSON file does not make its contents
a new format.

When Exile needs additional metadata or capabilities, prefer a documented
container, schema, or versioned specification with a clear compatibility story.

### 4.3 Keep identification and trust separate

Extensions and MIME types are hints, not security boundaries. Content may be
misnamed, malformed, truncated, or deliberately crafted.

- Do not execute files merely because their extension is recognized.
- Validate structured data before consuming it.
- Validate paths and resource limits before extracting archives.
- Verify package integrity and publisher authorization before installation.
- Keep executable permissions and privileged operations under explicit system
  policy rather than trusting filenames.

Where both filename and content identification are available, callers must
choose the appropriate validation policy for their task. A MIME guess alone
must never authorize execution or installation.

## 5. Exile Store package-format status

The current Exile Store development backend uses `.expkg`.

Its implementation and specification are in:

- `Userland/Applications/ExileStore/src/core/expkg.h`
- `Userland/Applications/ExileStore/src/core/expkg.cpp`

The current development container uses the `EXPKG1` header, a file-entry
listing, a `data` separator, and concatenated file bytes. It is intended for
regular files with validated relative paths, not as a general archive format.

The Store source explicitly describes `.expkg` as a development format and
states that the production Exile package format is a separate integration
point. The Store's `LocalPrefixBackend` is an unprivileged development
stand-in, not the production operating-system installer.

Therefore:

- Keep `.expkg` for its existing development workflow.
- Do not declare `.expkg` to be the final production package format.
- Do not treat proposed `.pkg` catalog URLs as proof of a finalized format.
- Do not introduce a second production package extension before the package
  specification, installer contract, signature policy, and migration plan are
  agreed upon.

A production package design must define versioning, metadata, payload layout,
integrity verification, publisher authentication, rollback policy, resource
limits, safe extraction or installation, and compatibility with the system
installer. It must also be integrated and tested before being described as
implemented.

## 6. Requirements for future Exile-native formats

A new native format may be proposed only when it provides a concrete benefit,
such as stronger integrity, predictable cross-device behavior, structured
metadata, efficient incremental updates, or a well-defined application
workflow.

Every proposal must document:

1. Purpose and intended users.
2. Extension and MIME identifier, including any registration requirements.
3. File signature or other reliable identification method where applicable.
4. Version field and rules for backward and forward compatibility.
5. Data model, encoding, byte order, and character encoding where relevant.
6. Parser limits and handling of malformed or hostile inputs.
7. Integrity and authenticity requirements where applicable.
8. Privacy implications and whether metadata can expose sensitive information.
9. Reference implementation and automated tests.
10. Applications and services that actually read or write the format.
11. Import, export, and migration paths where interoperability matters.
12. Licensing and provenance of any reused specification or implementation.

A proposal is not an implemented feature until the relevant code, tests,
documentation, and application integration exist.

## 7. Registry and association maintenance

When adding or changing a format:

- Update the central MIME registry where appropriate.
- Use a consistent extension convention and case-insensitive matching where
  the platform's path semantics require it.
- Add tests for expected extensions, mixed-case names, unknown extensions,
  and special filenames.
- Add content-signature tests when the registry recognizes signatures.
- Review overlaps and ordering where multiple formats share signatures.
- Update file associations only when the relevant icon or application exists.
- Document whether support is recognition-only, read-only, read/write, or
  installation-only.
- Preserve existing upstream copyright notices and SPDX licensing.

Special filenames such as `CMakeLists.txt` and dotfiles such as `.history`
should be matched as complete basenames where intended, rather than accidentally
being treated as arbitrary suffixes. Extension matching must also avoid
unintended matches against directory names or unrelated trailing text.

MIME registration must not claim more than the implementation can guarantee.
Unknown or unrecognized files should remain identifiable as unknown data,
rather than being assigned a fabricated Exile format.

## 8. Implementation roadmap

### Phase A — Inventory and correctness

- Audit MIME entries against actual readers, writers, and file associations.
- Review extension matching and special-filename behavior.
- Add focused tests for case handling, known formats, and unknown formats.
- Identify duplicate or ambiguous registrations and document intentional cases.

### Phase B — Application integration

- Map each recognized format to an existing application or a clear fallback.
- Separate icon availability from actual open/edit support.
- Ensure unsupported files produce an honest, useful user-facing message.

### Phase C — Production package integration

- Define the production package format with the system installer.
- Define signature, trust-root, versioning, and rollback requirements.
- Replace the Store's development backend only when the real OS service is
  available and its security contract is implemented.
- Keep development fixtures clearly labelled as such.

### Phase D — Purpose-built native formats

- Evaluate concrete Exile-specific needs before creating a new format.
- Publish specifications and compatibility rules before relying on them.
- Implement parsers, writers, tests, and application support together.
- Preserve import/export paths when a proprietary or platform-specific format
  would otherwise create unnecessary lock-in.

## 9. Definition of done

A file-format feature is complete only when its claimed capabilities are
accurate, its behavior is documented, malformed input is handled deliberately,
tests cover important positive and negative cases, and the relevant application
or service integration exists.

This document records repository findings and policy. It does not itself
implement new formats, guarantee support for every listed format, or certify
the system as production-secure.
