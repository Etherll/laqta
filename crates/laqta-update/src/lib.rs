//! Update check and install (M6 D12): the latest GitHub release of the Laqta repository; on
//! Windows the Velopack package is fetched through `laqta-http`'s verified fetch (hash from the
//! release's `SHA256SUMS`) and applied with Velopack's `Update.exe`; on Linux the caller shows a notice.
