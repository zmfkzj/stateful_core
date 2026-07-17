# Changelog

## [1.0.0](https://github.com/zmfkzj/stateful_core/compare/stateful-store-v0.1.1...stateful-store-v1.0.0) (2026-07-17)


### Features

* add atomic event journal and projectors ([7db126b](https://github.com/zmfkzj/stateful_core/commit/7db126b24f0fbc58a515d6b172995eacc8def9f4))
* add exact-read freshness and thin safety ([6a67680](https://github.com/zmfkzj/stateful_core/commit/6a676808b6114089e60479adf730931942e27cfe))
* add OMP sandbox tooling, external command policy, and DeNovo coverage ([4e1869e](https://github.com/zmfkzj/stateful_core/commit/4e1869ef4ea0fc33d7f3dc1f930232476dc83cb7))
* authorize writes by reservation id ([5a83bda](https://github.com/zmfkzj/stateful_core/commit/5a83bda2cd143f24dd0a20e92a525700438a5cb2))
* complete presence-first v2 cutover ([329f9d5](https://github.com/zmfkzj/stateful_core/commit/329f9d54e698ce270e62a37b7206077015df445a))
* cut server protocol to stateful v2 ([bfc27a1](https://github.com/zmfkzj/stateful_core/commit/bfc27a106f2f31eb39a05f4369b544a474e1ede0))
* define stateful v2 domain contracts ([ffbd994](https://github.com/zmfkzj/stateful_core/commit/ffbd9940bab7f30a85982f531ba91f67ac522e19))
* deliver versioned coordination context ([8657b1f](https://github.com/zmfkzj/stateful_core/commit/8657b1f0b591ab02ef917d5bc021cfa3b19ba553))
* event-source coordination aggregates ([8e65397](https://github.com/zmfkzj/stateful_core/commit/8e653976aa3f8d45a4a199b779314dce676f0df9))
* finalize reservation id authorization ([ba976de](https://github.com/zmfkzj/stateful_core/commit/ba976de86e1129d28a604075a9fda8f9cd2dc789))
* finalize reservation id authorization ([b8af2e1](https://github.com/zmfkzj/stateful_core/commit/b8af2e1edf5ac997000c33eebb547c05ad2a150e))
* improve OMP sandbox and DeNovo Docker workflows ([9f507f7](https://github.com/zmfkzj/stateful_core/commit/9f507f72fdde34781c8dff796ccede94840026e4))
* **mcp:** batch claim acquisition ([2abbba7](https://github.com/zmfkzj/stateful_core/commit/2abbba7a066dff24594ddfd00cf1ca7ad2c8d833))
* migrate legacy state into journal seeds ([308a66f](https://github.com/zmfkzj/stateful_core/commit/308a66fc9f42b6905e702ad643a269e3a559335f))
* migrate stateful cli clients to v2 ([2b95351](https://github.com/zmfkzj/stateful_core/commit/2b95351a72a0c307b4acd3f91755069cdfd1a4c9))
* **omp:** improve sandbox tool UX and reservation recovery ([a10dfa7](https://github.com/zmfkzj/stateful_core/commit/a10dfa70b2e4f6c2db4a18ec9ab045e4179e23ac))
* persist presence and structured handoffs ([f7ec9f5](https://github.com/zmfkzj/stateful_core/commit/f7ec9f53ee9f1a5ea524144eb3c2b54b8f383ea5))
* store reservation ids on claims ([3eda406](https://github.com/zmfkzj/stateful_core/commit/3eda4064d1777f2d870b9e937360105383b63cda))


### Bug Fixes

* address final v2 verification findings ([8f39d2f](https://github.com/zmfkzj/stateful_core/commit/8f39d2ff083dad751bef72610f97cb0516459ffd))
* atomically project write lifecycle presence ([8ecdf01](https://github.com/zmfkzj/stateful_core/commit/8ecdf01d3187fe9b0eabc9c36aa0dda6dc8ee186))
* audit whitespace authorization denials ([1c150c6](https://github.com/zmfkzj/stateful_core/commit/1c150c674edc809f1b1dd25ed1d8310d2148c565))
* bind authorize receipts to requests ([3146ca6](https://github.com/zmfkzj/stateful_core/commit/3146ca6131e37958d302bbcd7f9b74ee2232f0ec))
* bind context cursor to event sequence ([babd541](https://github.com/zmfkzj/stateful_core/commit/babd5416b99499695fb2518e9f82dbb87388ec8f))
* bind lifecycle resources to journal state ([24e2837](https://github.com/zmfkzj/stateful_core/commit/24e2837b3a487e2fa1b46320405cc409ade148d8))
* close aggregate lifecycle gaps ([9b5d96b](https://github.com/zmfkzj/stateful_core/commit/9b5d96ba7c862a11268af85394c38f306409b712))
* close aggregate lifecycle gaps ([940f664](https://github.com/zmfkzj/stateful_core/commit/940f66413ee0f157311a544c5ab4f206983dd87e))
* close benchmark final review findings ([280ca99](https://github.com/zmfkzj/stateful_core/commit/280ca99f1e1452367da52b103deb0c773f02661c))
* close v2 delivery and recovery gaps ([7a18baf](https://github.com/zmfkzj/stateful_core/commit/7a18baf47f5560b26bde080148d1176b17a9c8c0))
* close v2 server lifecycle gaps ([2fa8af6](https://github.com/zmfkzj/stateful_core/commit/2fa8af67fc742db2fcc46091397b5d4f7813f909))
* coalesce presence expiry maintenance ([9de84e3](https://github.com/zmfkzj/stateful_core/commit/9de84e3ba39d574a1c55a4c5aa6f2b9bbca28848))
* compare presence expiry instants ([e5e125f](https://github.com/zmfkzj/stateful_core/commit/e5e125f94aa19a88c386cf15da05448b52311301))
* complete event-sourced aggregate cutover ([9a80cd9](https://github.com/zmfkzj/stateful_core/commit/9a80cd908ffcefa8ead3789df2366786ecba8700))
* deduplicate migrated context scopes ([5564273](https://github.com/zmfkzj/stateful_core/commit/5564273a314329edc2b05014b88c34ac99cd1069))
* enforce fence ownership boundaries ([cb0b11a](https://github.com/zmfkzj/stateful_core/commit/cb0b11afb6e447c95cf36a50518bac1b8e6a5b57))
* enforce presence expiry before reads ([a7f3cff](https://github.com/zmfkzj/stateful_core/commit/a7f3cffb97143dfe74d7715d151f541d97028d3b))
* enforce v2 server contracts ([d674841](https://github.com/zmfkzj/stateful_core/commit/d674841bb8e93f02ee8aefcd6fc88ebd2806a7c5))
* exclude terminal claims from migrated authority ([5ea0749](https://github.com/zmfkzj/stateful_core/commit/5ea07490300a9d71927e60ca61d5d92885eb41b7))
* expire stalled write intents ([0aec11e](https://github.com/zmfkzj/stateful_core/commit/0aec11e3efeed13c5b26a7586571600cc2f7992f))
* flatten authorization warning audit ([9cc610d](https://github.com/zmfkzj/stateful_core/commit/9cc610d461b3f9b0b1ff6c7c2a545b0aa6bfa1c1))
* freeze rejections and preserve migration terminals ([ea15822](https://github.com/zmfkzj/stateful_core/commit/ea158229f97a26e7520f7bc68ac2f65a22d3d229))
* freeze server authorization denials ([d8d9ede](https://github.com/zmfkzj/stateful_core/commit/d8d9ede6fa128bd6c7e60cb6c8edf42cd520044a))
* harden journal receipts and replay ([edbf9f7](https://github.com/zmfkzj/stateful_core/commit/edbf9f7b273e68bc583329e5c71863c7e8f30472))
* harden legacy migration cutover ([31088ad](https://github.com/zmfkzj/stateful_core/commit/31088ad56fad207d6c5db29812d62b3ad2f7244e))
* harden presence and handoff lifecycle ([6514e23](https://github.com/zmfkzj/stateful_core/commit/6514e2311d3619bbeed5608d26b79e9278ae7d79))
* harden unknown write reconciliation ([2756c82](https://github.com/zmfkzj/stateful_core/commit/2756c8255190ec204ab2a416531ff034b25321c0))
* harden versioned context delivery ([8a0b81e](https://github.com/zmfkzj/stateful_core/commit/8a0b81e71be6265592fd9079cb03b996bc69a488))
* make legacy migration ordering portable ([a8c1479](https://github.com/zmfkzj/stateful_core/commit/a8c1479c31c7756f143bf0bc2bf625691bca8632))
* make notification poll receipted ([ee05a10](https://github.com/zmfkzj/stateful_core/commit/ee05a10063694338ac81fc837dd4c2bb9b96b500))
* normalize migrated active ordering ([9f5da7e](https://github.com/zmfkzj/stateful_core/commit/9f5da7e17d8e295f9553d2f9acecfb22b9436e8c))
* normalize migrated active ordering ([ac0c3f0](https://github.com/zmfkzj/stateful_core/commit/ac0c3f0b843e3b60f54bd1eaa53bb97c75b308e5))
* normalize migrated directory waits ([7c1dbe1](https://github.com/zmfkzj/stateful_core/commit/7c1dbe122c8d3b79b90fa5e778e2bd0cb45ba2a0))
* preserve aggregate lifecycle semantics ([3200d7b](https://github.com/zmfkzj/stateful_core/commit/3200d7bb5f2807d2c0048dcb3fc8b1da40ad863f))
* preserve expiry and observation causality ([b7079e6](https://github.com/zmfkzj/stateful_core/commit/b7079e6a6a3b60caba542634ee8f19f4c6903d42))
* preserve legacy event sequence provenance ([7f84ec0](https://github.com/zmfkzj/stateful_core/commit/7f84ec0a1e7b78abd7f6058ef4207d48d6685e4f))
* preserve task reservation authority ([cd7f603](https://github.com/zmfkzj/stateful_core/commit/cd7f60341ba7f43419ad39bab2cc6d81d5b0b019))
* preserve v2 freshness and heartbeat safety ([ac13ef3](https://github.com/zmfkzj/stateful_core/commit/ac13ef360f97187a488d981123ed9570452a4e94))
* preserve write freshness ownership ([0b2f937](https://github.com/zmfkzj/stateful_core/commit/0b2f9379900ea46d98d772b37267845907b08535))
* prioritize actionable context delivery ([80094ca](https://github.com/zmfkzj/stateful_core/commit/80094ca9170825bf37e123003a8e803fe8d61f22))
* restore exact read freshness contract ([7c66c06](https://github.com/zmfkzj/stateful_core/commit/7c66c06f84eb4874914184c21b5ddadd297fb52c))
* return authorize warning decisions ([4e53198](https://github.com/zmfkzj/stateful_core/commit/4e53198f4b2f3040a8bdc1b69438bb6adb71367a))
* **store:** close presence handoff lifecycles ([e44e925](https://github.com/zmfkzj/stateful_core/commit/e44e9252ba3c102952a0647cca0b72b21ffaa03a))
* **store:** protect activity identity ([a68ebfa](https://github.com/zmfkzj/stateful_core/commit/a68ebfa36b3a38dcc2fb668e807bb8b4ac5c31ea))
* tighten reservation scoped claims ([3a9953a](https://github.com/zmfkzj/stateful_core/commit/3a9953ade6b8040c7ff3965522df7143b0892949))
* update ProgramBench OMP lifecycle ([cdb4d97](https://github.com/zmfkzj/stateful_core/commit/cdb4d9726dc8271e3cfa4c2e761a253b529ca021))
* validate frozen receipts and repair terminal projections ([18d2eae](https://github.com/zmfkzj/stateful_core/commit/18d2eae229d2dba70f38656456017d9de02a3f38))
* validate persisted journal envelopes ([25fc62b](https://github.com/zmfkzj/stateful_core/commit/25fc62bdc68a77a1a1fa92eeeffb2395d716d62f))


### Miscellaneous Chores

* release 1.0.0 ([97745c7](https://github.com/zmfkzj/stateful_core/commit/97745c77ffe77543b94a372fdb7861df2b854ea7))

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
