# Task: Add missing trait implementations to stdlib types

## Summary of Changes

### 1. fs.rs — `std::io::Read + Write + Seek` for `fs::VumaFile`
- Added `impl std::io::Read for VumaFile` — delegates to inner `std::fs::File`
- Added `impl std::io::Write for VumaFile` — delegates to inner `std::fs::File`
- Added `impl std::io::Seek for VumaFile` — delegates to inner `std::fs::File`
- Updated imports to include `Seek as StdSeek` and `SeekFrom`

### 2. fs.rs — `From` conversions for `fs::VumaIoError`
- Added `impl From<std::io::Error> for VumaIoError` — maps `std::io::ErrorKind` → `VumaErrorKind`
- Added `impl From<VumaIoError> for std::io::Error` — maps `VumaErrorKind` → `std::io::ErrorKind`

### 3. error.rs — `From<VumaErrorChain> for std::io::Error`
- Added bidirectional conversion: `VumaErrorChain` → `std::io::Error` mapping error kinds appropriately

### 4. io.rs — Cross-domain `From` conversions for `io::VumaIoError`
- Added `impl From<crate::thread::VumaThreadError> for VumaIoError`
- Added `impl From<crate::env::VumaEnvError> for VumaIoError`
- Added `impl From<crate::fs::VumaIoError> for VumaIoError`

### 5. Tests Added
- `error.rs`: `test_from_error_chain_to_std_io_error` — verifies `VumaErrorChain → std::io::Error` conversion
- `fs.rs`: `test_vuma_file_std_read_trait` — verifies `std::io::Read` on `fs::VumaFile`
- `fs.rs`: `test_vuma_file_std_write_trait` — verifies `std::io::Write` on `fs::VumaFile`
- `fs.rs`: `test_vuma_file_std_seek_trait` — verifies `std::io::Seek` on `fs::VumaFile`
- `fs.rs`: `test_vuma_io_error_from_std_io_error` — verifies `From<std::io::Error>` for `fs::VumaIoError`
- `fs.rs`: `test_vuma_io_error_into_std_io_error` — verifies `From<fs::VumaIoError>` for `std::io::Error`
- `io.rs`: `test_from_thread_error_to_vuma_io_error` — verifies thread → I/O error conversion
- `io.rs`: `test_from_env_error_to_vuma_io_error` — verifies env → I/O error conversion
- `io.rs`: `test_from_fs_io_error_to_vuma_io_error` — verifies fs → I/O error conversion

## Verification
- `cargo check -p vuma-std` passes cleanly with no errors or warnings.

## Pre-existing trait implementations (confirmed complete)
All public error types already had `Display`, `Debug`, `Clone`, and `std::error::Error`:
- `VumaErrorChain`, `VumaErrorKind` (error.rs)
- `VumaIoError` (fs.rs)
- `VumaIoError`, `VumaIoErrorKind` (io.rs)
- `VumaThreadError` (thread.rs)
- `VumaEnvError` (env.rs)

All I/O types in `io.rs` already had proper `std::io` trait implementations:
- `VumaStdin` ✅ `std::io::Read`
- `VumaStdout` ✅ `std::io::Write`
- `VumaStderr` ✅ `std::io::Write`
- `VumaFile` (io) ✅ `std::io::Read + Write + Seek`
