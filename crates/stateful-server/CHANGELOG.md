# Changelog

## [1.0.0](https://github.com/zmfkzj/stateful_core/releases/tag/stateful-server-v1.0.0) (2026-07-17)


### Features

* add exact-read freshness and thin safety ([6a67680](https://github.com/zmfkzj/stateful_core/commit/6a676808b6114089e60479adf730931942e27cfe))
* add OMP sandbox tooling, external command policy, and DeNovo coverage ([4e1869e](https://github.com/zmfkzj/stateful_core/commit/4e1869ef4ea0fc33d7f3dc1f930232476dc83cb7))
* authorize writes by reservation id ([5a83bda](https://github.com/zmfkzj/stateful_core/commit/5a83bda2cd143f24dd0a20e92a525700438a5cb2))
* complete presence-first v2 cutover ([329f9d5](https://github.com/zmfkzj/stateful_core/commit/329f9d54e698ce270e62a37b7206077015df445a))
* cut server protocol to stateful v2 ([bfc27a1](https://github.com/zmfkzj/stateful_core/commit/bfc27a106f2f31eb39a05f4369b544a474e1ede0))
* define stateful v2 domain contracts ([ffbd994](https://github.com/zmfkzj/stateful_core/commit/ffbd9940bab7f30a85982f531ba91f67ac522e19))
* finalize reservation id authorization ([ba976de](https://github.com/zmfkzj/stateful_core/commit/ba976de86e1129d28a604075a9fda8f9cd2dc789))
* finalize reservation id authorization ([b8af2e1](https://github.com/zmfkzj/stateful_core/commit/b8af2e1edf5ac997000c33eebb547c05ad2a150e))
* improve OMP sandbox and DeNovo Docker workflows ([9f507f7](https://github.com/zmfkzj/stateful_core/commit/9f507f72fdde34781c8dff796ccede94840026e4))
* make OMP bash tools background by default ([618d641](https://github.com/zmfkzj/stateful_core/commit/618d641725e3cc578eafeeb6bceaa3687a734fff))
* **mcp:** batch claim acquisition ([2abbba7](https://github.com/zmfkzj/stateful_core/commit/2abbba7a066dff24594ddfd00cf1ca7ad2c8d833))
* **omp:** improve sandbox tool UX and reservation recovery ([a10dfa7](https://github.com/zmfkzj/stateful_core/commit/a10dfa70b2e4f6c2db4a18ec9ab045e4179e23ac))


### Bug Fixes

* address final v2 verification findings ([8f39d2f](https://github.com/zmfkzj/stateful_core/commit/8f39d2ff083dad751bef72610f97cb0516459ffd))
* allow-directory-lease-observations ([c23ea85](https://github.com/zmfkzj/stateful_core/commit/c23ea85fcc0eb13dc7c894dd39237fc747d0b22a))
* atomically project write lifecycle presence ([8ecdf01](https://github.com/zmfkzj/stateful_core/commit/8ecdf01d3187fe9b0eabc9c36aa0dda6dc8ee186))
* audit whitespace authorization denials ([1c150c6](https://github.com/zmfkzj/stateful_core/commit/1c150c674edc809f1b1dd25ed1d8310d2148c565))
* bind authorize receipts to requests ([3146ca6](https://github.com/zmfkzj/stateful_core/commit/3146ca6131e37958d302bbcd7f9b74ee2232f0ec))
* bind lifecycle resources to journal state ([24e2837](https://github.com/zmfkzj/stateful_core/commit/24e2837b3a487e2fa1b46320405cc409ade148d8))
* bind runtime endpoint to process pid ([cb0adce](https://github.com/zmfkzj/stateful_core/commit/cb0adce20b935dd4b0f47d0a70af8d6857cf2b2b))
* close v2 delivery and recovery gaps ([7a18baf](https://github.com/zmfkzj/stateful_core/commit/7a18baf47f5560b26bde080148d1176b17a9c8c0))
* close v2 server lifecycle gaps ([2fa8af6](https://github.com/zmfkzj/stateful_core/commit/2fa8af67fc742db2fcc46091397b5d4f7813f909))
* enforce v2 domain invariants ([68265b5](https://github.com/zmfkzj/stateful_core/commit/68265b51b4b281251d16b43ad3fafd8105dde30b))
* enforce v2 server contracts ([d674841](https://github.com/zmfkzj/stateful_core/commit/d674841bb8e93f02ee8aefcd6fc88ebd2806a7c5))
* freeze server authorization denials ([d8d9ede](https://github.com/zmfkzj/stateful_core/commit/d8d9ede6fa128bd6c7e60cb6c8edf42cd520044a))
* make notification poll receipted ([ee05a10](https://github.com/zmfkzj/stateful_core/commit/ee05a10063694338ac81fc837dd4c2bb9b96b500))
* preserve expiry and observation causality ([b7079e6](https://github.com/zmfkzj/stateful_core/commit/b7079e6a6a3b60caba542634ee8f19f4c6903d42))
* preserve mixed reservation operation authority ([61b9854](https://github.com/zmfkzj/stateful_core/commit/61b9854191292b890fadabc56ecc2642ea644eee))
* preserve task reservation authority ([cd7f603](https://github.com/zmfkzj/stateful_core/commit/cd7f60341ba7f43419ad39bab2cc6d81d5b0b019))
* preserve v2 freshness and heartbeat safety ([ac13ef3](https://github.com/zmfkzj/stateful_core/commit/ac13ef360f97187a488d981123ed9570452a4e94))
* preserve write freshness ownership ([0b2f937](https://github.com/zmfkzj/stateful_core/commit/0b2f9379900ea46d98d772b37267845907b08535))
* replace hardcoded home paths ([ff2db8b](https://github.com/zmfkzj/stateful_core/commit/ff2db8b5b5adfa53a328c36d0f9b83c4261bc804))
* return authorize warning decisions ([4e53198](https://github.com/zmfkzj/stateful_core/commit/4e53198f4b2f3040a8bdc1b69438bb6adb71367a))
* tighten reservation scoped claims ([3a9953a](https://github.com/zmfkzj/stateful_core/commit/3a9953ade6b8040c7ff3965522df7143b0892949))
* update ProgramBench OMP lifecycle ([cdb4d97](https://github.com/zmfkzj/stateful_core/commit/cdb4d9726dc8271e3cfa4c2e761a253b529ca021))


### Miscellaneous Chores

* release 1.0.0 ([97745c7](https://github.com/zmfkzj/stateful_core/commit/97745c77ffe77543b94a372fdb7861df2b854ea7))

## 0.1.1 (unpublished) (2026-06-23)


### Features

* add explicit intent scheduling api ([70b9ad4](https://github.com/zmfkzj/stateful_core/commit/70b9ad4e4ec6d0f7f4804aead5d3f01b53f21d7d))
* add LAN runtime commands ([f29f48c](https://github.com/zmfkzj/stateful_core/commit/f29f48caf126d02e7813fd216275dc091dd2c20f))
* add live current state purpose tracking ([f8bacc2](https://github.com/zmfkzj/stateful_core/commit/f8bacc27d2f7f950db9fcb80dd86dd12f67044e9))
* add OMP sandbox tooling, external command policy, and DeNovo coverage ([4e1869e](https://github.com/zmfkzj/stateful_core/commit/4e1869ef4ea0fc33d7f3dc1f930232476dc83cb7))
* add v1 protocol envelope foundation ([691acf1](https://github.com/zmfkzj/stateful_core/commit/691acf1e8afefda29166ab04c9e8b76616a2bf3f))
* harden stateful command coordination ([0dc621a](https://github.com/zmfkzj/stateful_core/commit/0dc621a776794a6f383c56fe646596445176dece))
* harden stateful coordination and sandboxed command execution ([10c2109](https://github.com/zmfkzj/stateful_core/commit/10c2109dc59a1e8cf05dbe227427259b00c3cb2d))
* harden stateful coordination for release ([b1c7413](https://github.com/zmfkzj/stateful_core/commit/b1c741353c6f79f58684ce3f273e22f357ac9ed2))
* render minimal coordination context ([8dd1bb9](https://github.com/zmfkzj/stateful_core/commit/8dd1bb9537541c7985cf37eea90d101e1a76190c))
* require approved external writes ([a3118f3](https://github.com/zmfkzj/stateful_core/commit/a3118f3104ae031b2ca4d01f6824865b8672b173))
* require protocol metadata for intent declarations ([87eef46](https://github.com/zmfkzj/stateful_core/commit/87eef469339a9735a3cc9e3babb6a81e6f44f2ce))
* route authorize through policy service ([6151a54](https://github.com/zmfkzj/stateful_core/commit/6151a54081c66c3dd15cc164acd2f2295e506fcd))
* route write authorization through policy service ([9710abb](https://github.com/zmfkzj/stateful_core/commit/9710abb20556f6125f99472aee79388fdcc5ee6d))
* send protocol metadata across stateful clients ([79f1197](https://github.com/zmfkzj/stateful_core/commit/79f1197b828ad745451350ccdafda15b8f90446c))


### Bug Fixes

* address major coordination review findings ([6690be9](https://github.com/zmfkzj/stateful_core/commit/6690be952a1bad67c68a9a57b90e4585dd7ca48f))
* allow-directory-lease-observations ([c23ea85](https://github.com/zmfkzj/stateful_core/commit/c23ea85fcc0eb13dc7c894dd39237fc747d0b22a))
* clear finalized session intents ([cf47168](https://github.com/zmfkzj/stateful_core/commit/cf47168e27df68e3012efb0eea53f2f8b1239b49))
* clear finalized session intents ([8e26f2b](https://github.com/zmfkzj/stateful_core/commit/8e26f2ba70b09b2cc7ba5d0281803d2f4db4d1cf))
* close prototype enforcement gaps ([be85b58](https://github.com/zmfkzj/stateful_core/commit/be85b58b71d0f3a22678cb7fb81dcbe464a0a553))
* handle malformed protocol metadata ([10f6c72](https://github.com/zmfkzj/stateful_core/commit/10f6c72299813afa513b81d266ca48e71e5b7e82))
* preserve current-state identity across queued workflows ([b368a45](https://github.com/zmfkzj/stateful_core/commit/b368a454c82afe5a3e698273a4a88782b854b04b))
* preserve LAN runtime and isolate shared context ([caf8ef3](https://github.com/zmfkzj/stateful_core/commit/caf8ef35088d0a508f0b2f9931ee85129674f8e0))
* preserve protocol callers and authorize queue retries ([b085d73](https://github.com/zmfkzj/stateful_core/commit/b085d73e747cb0e8ae001962f05232555c94c7f8))
* preserve queued workflow identity context ([dfd19a9](https://github.com/zmfkzj/stateful_core/commit/dfd19a9bec19dff6d8cf63e9d3dd18299dd88685))
* preserve queued workflow identity in production paths ([ef3438d](https://github.com/zmfkzj/stateful_core/commit/ef3438da177b8fc48d76ad0c58e26667add63fc3))
* publish server runtime after bind ([cb2587e](https://github.com/zmfkzj/stateful_core/commit/cb2587e912842c817375874fce15d2304bc5866b))
* validate protocol observed_at timestamp ([b715c8f](https://github.com/zmfkzj/stateful_core/commit/b715c8f96903c90fa2c5928abc072cbc092d6b9b))
* validate v1 protocol envelope metadata ([fa09376](https://github.com/zmfkzj/stateful_core/commit/fa0937690efc5b44c18014d006ffaf4ea571ccca))
* verify server identity and start options ([4468f07](https://github.com/zmfkzj/stateful_core/commit/4468f070de3f4afc5db19e0056e1abf6da49b0b0))
