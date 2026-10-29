# Dependency license supplements

Some crates in `Cargo.lock` omit license files from their published archives.
The files in this directory supplement those archives for binary distribution.
`manifest.json` maps exact package versions to original texts, source URLs pinned
to commits, the selected license, and a SHA-256 checksum. Identical upstream
texts are shared to avoid duplicate files.

The package's `.cargo_vcs_info.json` supplies the source commit. Whenever the
upstream repository includes a complete license at that commit, its bytes are
retained unchanged. Several packages declare MIT only in `Cargo.toml`, or link
to the MIT terms from a license notice. For those, both the original declaration
and the standard MIT text from the official SPDX license list are retained. The
manifest identifies these as separate declarations and standard terms; no
package-specific copyright line has been fabricated. The collected inventory
also records the authors declared in each published package's metadata.

The collector runs offline and checks every supplement's checksum. Updating a
dependency does not automatically reuse a previous version's supplement: the
new source commit must be checked and mapped explicitly if it still omits a
license file. Standard MIT text is a reference to the terms, not a substitute
for retaining copyright notices included by upstream authors.
