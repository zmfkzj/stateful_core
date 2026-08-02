# Changelog

## [0.1.2](https://github.com/zmfkzj/stateful_core/compare/stateful-store-v0.1.1...stateful-store-v0.1.2) (2026-08-02)


### Features

* add explicit intent scheduling api ([70b9ad4](https://github.com/zmfkzj/stateful_core/commit/70b9ad4e4ec6d0f7f4804aead5d3f01b53f21d7d))
* add LAN runtime commands ([f29f48c](https://github.com/zmfkzj/stateful_core/commit/f29f48caf126d02e7813fd216275dc091dd2c20f))
* add lazy expiration with injectable clock ([7af14ce](https://github.com/zmfkzj/stateful_core/commit/7af14ce47f53a98e07cb29dfcf6da3c92ad2fd25))
* add live current state purpose tracking ([f8bacc2](https://github.com/zmfkzj/stateful_core/commit/f8bacc27d2f7f950db9fcb80dd86dd12f67044e9))
* add OMP sandbox tooling, external command policy, and DeNovo coverage ([4e1869e](https://github.com/zmfkzj/stateful_core/commit/4e1869ef4ea0fc33d7f3dc1f930232476dc83cb7))
* authorize writes by reservation id ([5a83bda](https://github.com/zmfkzj/stateful_core/commit/5a83bda2cd143f24dd0a20e92a525700438a5cb2))
* finalize reservation id authorization ([ba976de](https://github.com/zmfkzj/stateful_core/commit/ba976de86e1129d28a604075a9fda8f9cd2dc789))
* finalize reservation id authorization ([b8af2e1](https://github.com/zmfkzj/stateful_core/commit/b8af2e1edf5ac997000c33eebb547c05ad2a150e))
* harden stateful coordination and sandboxed command execution ([10c2109](https://github.com/zmfkzj/stateful_core/commit/10c2109dc59a1e8cf05dbe227427259b00c3cb2d))
* harden stateful coordination for release ([b1c7413](https://github.com/zmfkzj/stateful_core/commit/b1c741353c6f79f58684ce3f273e22f357ac9ed2))
* improve OMP sandbox and DeNovo Docker workflows ([9f507f7](https://github.com/zmfkzj/stateful_core/commit/9f507f72fdde34781c8dff796ccede94840026e4))
* **mcp:** batch claim acquisition ([2abbba7](https://github.com/zmfkzj/stateful_core/commit/2abbba7a066dff24594ddfd00cf1ca7ad2c8d833))
* **omp:** improve sandbox tool UX and reservation recovery ([a10dfa7](https://github.com/zmfkzj/stateful_core/commit/a10dfa70b2e4f6c2db4a18ec9ab045e4179e23ac))
* persist idempotent intent requests ([4edbaf3](https://github.com/zmfkzj/stateful_core/commit/4edbaf353e52e48d207efa11aec7c7a702dcecec))
* render minimal coordination context ([8dd1bb9](https://github.com/zmfkzj/stateful_core/commit/8dd1bb9537541c7985cf37eea90d101e1a76190c))
* require approved external writes ([a3118f3](https://github.com/zmfkzj/stateful_core/commit/a3118f3104ae031b2ca4d01f6824865b8672b173))
* store repo identity on events ([2fea8b3](https://github.com/zmfkzj/stateful_core/commit/2fea8b333cde474e2ad91fe4a9c3c2a2ab168b0a))
* store reservation ids on claims ([3eda406](https://github.com/zmfkzj/stateful_core/commit/3eda4064d1777f2d870b9e937360105383b63cda))
* tighten stateful coordination safeguards ([aa3c153](https://github.com/zmfkzj/stateful_core/commit/aa3c1534adada17d05a0b90ae52f41993ce72863))


### Bug Fixes

* address major coordination review findings ([6690be9](https://github.com/zmfkzj/stateful_core/commit/6690be952a1bad67c68a9a57b90e4585dd7ca48f))
* clean legacy purpose-less coordination state ([e225210](https://github.com/zmfkzj/stateful_core/commit/e2252100017bc75d56333eac22626552adf20a02))
* clear finalized session intents ([cf47168](https://github.com/zmfkzj/stateful_core/commit/cf47168e27df68e3012efb0eea53f2f8b1239b49))
* clear finalized session intents ([8e26f2b](https://github.com/zmfkzj/stateful_core/commit/8e26f2ba70b09b2cc7ba5d0281803d2f4db4d1cf))
* harden intent request cancellation ([88531cc](https://github.com/zmfkzj/stateful_core/commit/88531ccd4cc1db8322eb888a743d60bc3a8e598d))
* preserve current-state identity across queued workflows ([b368a45](https://github.com/zmfkzj/stateful_core/commit/b368a454c82afe5a3e698273a4a88782b854b04b))
* preserve LAN runtime and isolate shared context ([caf8ef3](https://github.com/zmfkzj/stateful_core/commit/caf8ef35088d0a508f0b2f9931ee85129674f8e0))
* preserve queued workflow identity context ([dfd19a9](https://github.com/zmfkzj/stateful_core/commit/dfd19a9bec19dff6d8cf63e9d3dd18299dd88685))
* preserve queued workflow identity in production paths ([ef3438d](https://github.com/zmfkzj/stateful_core/commit/ef3438da177b8fc48d76ad0c58e26667add63fc3))
* rollback failed store appends ([298917f](https://github.com/zmfkzj/stateful_core/commit/298917f5f0e400c019737ea9bb3b55f863b71cdb))
* tighten reservation scoped claims ([3a9953a](https://github.com/zmfkzj/stateful_core/commit/3a9953ade6b8040c7ff3965522df7143b0892949))
* update ProgramBench OMP lifecycle ([cdb4d97](https://github.com/zmfkzj/stateful_core/commit/cdb4d9726dc8271e3cfa4c2e761a253b529ca021))
* 레거시 설치 마이그레이션을 복구한다 ([29c3203](https://github.com/zmfkzj/stateful_core/commit/29c320306d8dbac9ed1e024b307032da9221f161))

## [0.1.1](https://github.com/zmfkzj/stateful_core/compare/stateful-store-v0.1.0...stateful-store-v0.1.1) (2026-06-23)


### Features

* add explicit intent scheduling api ([70b9ad4](https://github.com/zmfkzj/stateful_core/commit/70b9ad4e4ec6d0f7f4804aead5d3f01b53f21d7d))
* add LAN runtime commands ([f29f48c](https://github.com/zmfkzj/stateful_core/commit/f29f48caf126d02e7813fd216275dc091dd2c20f))
* add lazy expiration with injectable clock ([7af14ce](https://github.com/zmfkzj/stateful_core/commit/7af14ce47f53a98e07cb29dfcf6da3c92ad2fd25))
* add live current state purpose tracking ([f8bacc2](https://github.com/zmfkzj/stateful_core/commit/f8bacc27d2f7f950db9fcb80dd86dd12f67044e9))
* add OMP sandbox tooling, external command policy, and DeNovo coverage ([4e1869e](https://github.com/zmfkzj/stateful_core/commit/4e1869ef4ea0fc33d7f3dc1f930232476dc83cb7))
* harden stateful coordination and sandboxed command execution ([10c2109](https://github.com/zmfkzj/stateful_core/commit/10c2109dc59a1e8cf05dbe227427259b00c3cb2d))
* harden stateful coordination for release ([b1c7413](https://github.com/zmfkzj/stateful_core/commit/b1c741353c6f79f58684ce3f273e22f357ac9ed2))
* persist idempotent intent requests ([4edbaf3](https://github.com/zmfkzj/stateful_core/commit/4edbaf353e52e48d207efa11aec7c7a702dcecec))
* render minimal coordination context ([8dd1bb9](https://github.com/zmfkzj/stateful_core/commit/8dd1bb9537541c7985cf37eea90d101e1a76190c))
* require approved external writes ([a3118f3](https://github.com/zmfkzj/stateful_core/commit/a3118f3104ae031b2ca4d01f6824865b8672b173))
* store repo identity on events ([2fea8b3](https://github.com/zmfkzj/stateful_core/commit/2fea8b333cde474e2ad91fe4a9c3c2a2ab168b0a))
* tighten stateful coordination safeguards ([aa3c153](https://github.com/zmfkzj/stateful_core/commit/aa3c1534adada17d05a0b90ae52f41993ce72863))


### Bug Fixes

* address major coordination review findings ([6690be9](https://github.com/zmfkzj/stateful_core/commit/6690be952a1bad67c68a9a57b90e4585dd7ca48f))
* clean legacy purpose-less coordination state ([e225210](https://github.com/zmfkzj/stateful_core/commit/e2252100017bc75d56333eac22626552adf20a02))
* clear finalized session intents ([cf47168](https://github.com/zmfkzj/stateful_core/commit/cf47168e27df68e3012efb0eea53f2f8b1239b49))
* clear finalized session intents ([8e26f2b](https://github.com/zmfkzj/stateful_core/commit/8e26f2ba70b09b2cc7ba5d0281803d2f4db4d1cf))
* harden intent request cancellation ([88531cc](https://github.com/zmfkzj/stateful_core/commit/88531ccd4cc1db8322eb888a743d60bc3a8e598d))
* preserve current-state identity across queued workflows ([b368a45](https://github.com/zmfkzj/stateful_core/commit/b368a454c82afe5a3e698273a4a88782b854b04b))
* preserve LAN runtime and isolate shared context ([caf8ef3](https://github.com/zmfkzj/stateful_core/commit/caf8ef35088d0a508f0b2f9931ee85129674f8e0))
* preserve queued workflow identity context ([dfd19a9](https://github.com/zmfkzj/stateful_core/commit/dfd19a9bec19dff6d8cf63e9d3dd18299dd88685))
* preserve queued workflow identity in production paths ([ef3438d](https://github.com/zmfkzj/stateful_core/commit/ef3438da177b8fc48d76ad0c58e26667add63fc3))
* rollback failed store appends ([298917f](https://github.com/zmfkzj/stateful_core/commit/298917f5f0e400c019737ea9bb3b55f863b71cdb))
