### Changed

- **The shell installer keeps each release in a directory of its own and switches to it in one step.**
  `install.sh` puts a release under `.crucible-install/releases/<version>` in the installation directory, with a receipt of what it holds, and links `crucible` and `cru` to the active one, so an interrupted install leaves the previous release in use or the new one complete. Earlier releases stay in place, and installing one of them again switches back to it. A version with a suffix or a leading zero, such as `1.2.3-rc.1`, is now refused.
