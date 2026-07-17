# Changelog

## [1.0.0](https://github.com/zmfkzj/stateful_core/compare/stateful-core-v0.1.1...stateful-core-v1.0.0) (2026-07-17)


### Features

* add exact-read freshness and thin safety ([6a67680](https://github.com/zmfkzj/stateful_core/commit/6a676808b6114089e60479adf730931942e27cfe))
* add OMP sandbox tooling, external command policy, and DeNovo coverage ([4e1869e](https://github.com/zmfkzj/stateful_core/commit/4e1869ef4ea0fc33d7f3dc1f930232476dc83cb7))
* define stateful v2 domain contracts ([ffbd994](https://github.com/zmfkzj/stateful_core/commit/ffbd9940bab7f30a85982f531ba91f67ac522e19))
* deliver versioned coordination context ([8657b1f](https://github.com/zmfkzj/stateful_core/commit/8657b1f0b591ab02ef917d5bc021cfa3b19ba553))
* finalize reservation id authorization ([ba976de](https://github.com/zmfkzj/stateful_core/commit/ba976de86e1129d28a604075a9fda8f9cd2dc789))
* improve OMP sandbox and DeNovo Docker workflows ([9f507f7](https://github.com/zmfkzj/stateful_core/commit/9f507f72fdde34781c8dff796ccede94840026e4))
* migrate legacy state into journal seeds ([308a66f](https://github.com/zmfkzj/stateful_core/commit/308a66fc9f42b6905e702ad643a269e3a559335f))
* persist presence and structured handoffs ([f7ec9f5](https://github.com/zmfkzj/stateful_core/commit/f7ec9f53ee9f1a5ea524144eb3c2b54b8f383ea5))


### Bug Fixes

* address final v2 verification findings ([8f39d2f](https://github.com/zmfkzj/stateful_core/commit/8f39d2ff083dad751bef72610f97cb0516459ffd))
* atomically project write lifecycle presence ([8ecdf01](https://github.com/zmfkzj/stateful_core/commit/8ecdf01d3187fe9b0eabc9c36aa0dda6dc8ee186))
* close aggregate lifecycle gaps ([940f664](https://github.com/zmfkzj/stateful_core/commit/940f66413ee0f157311a544c5ab4f206983dd87e))
* complete event-sourced aggregate cutover ([9a80cd9](https://github.com/zmfkzj/stateful_core/commit/9a80cd908ffcefa8ead3789df2366786ecba8700))
* enforce v2 domain invariants ([68265b5](https://github.com/zmfkzj/stateful_core/commit/68265b51b4b281251d16b43ad3fafd8105dde30b))
* harden legacy migration cutover ([31088ad](https://github.com/zmfkzj/stateful_core/commit/31088ad56fad207d6c5db29812d62b3ad2f7244e))
* harden unknown write reconciliation ([2756c82](https://github.com/zmfkzj/stateful_core/commit/2756c8255190ec204ab2a416531ff034b25321c0))
* harden versioned context delivery ([8a0b81e](https://github.com/zmfkzj/stateful_core/commit/8a0b81e71be6265592fd9079cb03b996bc69a488))
* preserve expiry and observation causality ([b7079e6](https://github.com/zmfkzj/stateful_core/commit/b7079e6a6a3b60caba542634ee8f19f4c6903d42))
* preserve v2 freshness and heartbeat safety ([ac13ef3](https://github.com/zmfkzj/stateful_core/commit/ac13ef360f97187a488d981123ed9570452a4e94))
* preserve write freshness ownership ([0b2f937](https://github.com/zmfkzj/stateful_core/commit/0b2f9379900ea46d98d772b37267845907b08535))
* prioritize actionable context delivery ([80094ca](https://github.com/zmfkzj/stateful_core/commit/80094ca9170825bf37e123003a8e803fe8d61f22))
* restore exact read freshness contract ([7c66c06](https://github.com/zmfkzj/stateful_core/commit/7c66c06f84eb4874914184c21b5ddadd297fb52c))
* update ProgramBench OMP lifecycle ([cdb4d97](https://github.com/zmfkzj/stateful_core/commit/cdb4d9726dc8271e3cfa4c2e761a253b529ca021))


### Miscellaneous Chores

* release 1.0.0 ([97745c7](https://github.com/zmfkzj/stateful_core/commit/97745c77ffe77543b94a372fdb7861df2b854ea7))

## [0.1.1](https://github.com/zmfkzj/stateful_core/compare/stateful-core-v0.1.0...stateful-core-v0.1.1) (2026-06-23)


### Features

* add live current state purpose tracking ([f8bacc2](https://github.com/zmfkzj/stateful_core/commit/f8bacc27d2f7f950db9fcb80dd86dd12f67044e9))
* add OMP sandbox tooling, external command policy, and DeNovo coverage ([4e1869e](https://github.com/zmfkzj/stateful_core/commit/4e1869ef4ea0fc33d7f3dc1f930232476dc83cb7))
* add sandboxed MCP bash writes ([ab1d6cf](https://github.com/zmfkzj/stateful_core/commit/ab1d6cff8725c6755c7de1670bc6c64a6d9a48df))
* add structured stateful commit ([3886b2f](https://github.com/zmfkzj/stateful_core/commit/3886b2fd1f212d922b7179af69a6a4c4f5854fa4))
* harden hooks and codex wrapper ([f7fbb64](https://github.com/zmfkzj/stateful_core/commit/f7fbb6402bafea6e4b0ac8e3468ea398ba11ae8e))
* harden stateful command coordination ([0dc621a](https://github.com/zmfkzj/stateful_core/commit/0dc621a776794a6f383c56fe646596445176dece))
* harden stateful coordination and sandboxed command execution ([10c2109](https://github.com/zmfkzj/stateful_core/commit/10c2109dc59a1e8cf05dbe227427259b00c3cb2d))
* improve stateful coordination tooling ([1846de4](https://github.com/zmfkzj/stateful_core/commit/1846de42cc2978e819ca3d93753b65cb83a02fb6))
* render minimal coordination context ([8dd1bb9](https://github.com/zmfkzj/stateful_core/commit/8dd1bb9537541c7985cf37eea90d101e1a76190c))
* require approved external writes ([a3118f3](https://github.com/zmfkzj/stateful_core/commit/a3118f3104ae031b2ca4d01f6824865b8672b173))
* route authorize through policy service ([6151a54](https://github.com/zmfkzj/stateful_core/commit/6151a54081c66c3dd15cc164acd2f2295e506fcd))


### Bug Fixes

* address major coordination review findings ([6690be9](https://github.com/zmfkzj/stateful_core/commit/6690be952a1bad67c68a9a57b90e4585dd7ca48f))
* close structured commit review gaps ([35eea66](https://github.com/zmfkzj/stateful_core/commit/35eea664439842ce3fbfbfcf9f952089d7d89f5e))
* harden structured commit validation ([c8b0926](https://github.com/zmfkzj/stateful_core/commit/c8b092696e9d43b7ecb1db1a67ddcc895b92e0ed))
* parse shell options before bash allowlists ([76f23f4](https://github.com/zmfkzj/stateful_core/commit/76f23f4504928662ece015465db235e57a51e21a))
* reject brace and glob shell expansions ([e8c3758](https://github.com/zmfkzj/stateful_core/commit/e8c3758711f58b09d9717639ed31a43f87e20076))
* reject broad stateful commit pathspecs ([524f207](https://github.com/zmfkzj/stateful_core/commit/524f207de491d5c97e8741973d342dbf3726c2aa))
* reject simple shell parameter expansions ([40cc96e](https://github.com/zmfkzj/stateful_core/commit/40cc96e0bf79595dd4b5c40bb43be1ed015ff9b0))
* reject unsupported shell expansions ([44f92cb](https://github.com/zmfkzj/stateful_core/commit/44f92cb4e798275e4d0295a1ed35c7fac80ef60c))
* satisfy stateful verification ([ee6ab2f](https://github.com/zmfkzj/stateful_core/commit/ee6ab2fbe6da85ff63b0ac0141484cd437acec1f))
* tighten bash read-only allowlists ([3570fd2](https://github.com/zmfkzj/stateful_core/commit/3570fd28500d01952eb941dca3581306e24ee1da))
* track bash quote state for expansions ([bf7508b](https://github.com/zmfkzj/stateful_core/commit/bf7508b26ab81b0cd806135a6909ca5b3a48aac0))
