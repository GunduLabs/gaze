# Persistence barriers and uncertain commits

Configuration writes synchronize file contents and mode before replacement, then
synchronize the containing directory before returning success. Temporary files
still use exclusive creation and ownership-scoped cleanup.

A directory-sync failure after replacement is not an ordinary failed save: the
new configuration is already visible, but its crash durability is unknown.
`ConfigCommitUncertain` distinguishes that condition. The template store must not
roll back its format as though the old configuration were still on disk. Instead
it disables matching/mutations and retains the staged old store until restart.
The D-Bus operation returns an error; restart gazed before retrying.

Startup still uses the existing format migration and staging-cleanup paths. This
patch is not a new journal or two-resource recovery protocol. Retained staging
copies are subject to existing startup cleanup; operators investigating an
uncertain commit should preserve a filesystem copy before restarting.

Duress marker creation synchronizes both the marker and its directory (including
the parent when establishing the marker directory). Clear synchronizes removal
before removing the in-memory lock. Failed synchronization retains a lock in the
running process. Create/clear operations are serialized by the memory mutex.

## Validation boundaries

The portable tests run production crypto/storage code with inert Spectrum/key-size
stand-ins, inject the uncertain-commit error at the persistence callback, and test
ordinary marker restart behavior. Config tests cover existing mode/comment/rename
retry guarantees. These are not power-cut tests.

Before closing review finding #13, fault-test file-sync, rename, directory-sync,
rollback, marker creation/removal, and encryption enable/disable on Linux, including
process death between store exchange and configuration replacement. Confirm startup
recovery on the supported filesystem and retain a recovery copy if migration fails.
