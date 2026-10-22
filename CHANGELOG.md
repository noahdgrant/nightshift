# Changelog

## [0.5.0](https://github.com/noahdgrant/nightshift/compare/v0.4.0...v0.5.0) (2026-10-22)


### Features

* **cli:** a unit waiting for a human merge is refreshed against main by ns run ([#907](https://github.com/noahdgrant/nightshift/issues/907)) ([1b372ee](https://github.com/noahdgrant/nightshift/commit/1b372ee71dd610e3b145b9ac12cb2eed903d82bc))
* **cli:** an inconclusive verification runs again, then sticks the unit for a human ([#888](https://github.com/noahdgrant/nightshift/issues/888)) ([748bba6](https://github.com/noahdgrant/nightshift/commit/748bba6b2edac8e0f3f2c7f8e2b4cafdd8b03309))
* **cli:** ns triage records the issues a unit is blocked by ([#898](https://github.com/noahdgrant/nightshift/issues/898)) ([a6ff09d](https://github.com/noahdgrant/nightshift/commit/a6ff09d5dd5e3c621fb2caae54d5a98f783f7a7a))
* **cli:** pick the model each step uses in nightshift.toml ([#904](https://github.com/noahdgrant/nightshift/issues/904)) ([f96dea2](https://github.com/noahdgrant/nightshift/commit/f96dea2ff374200f2aa798bb3b3a4f2d6dbdb18c))
* **cli:** read GitHub's blocked-by relationship in ns auto and ns run ([#895](https://github.com/noahdgrant/nightshift/issues/895)) ([4001d46](https://github.com/noahdgrant/nightshift/commit/4001d464c319e225145250994c8a4943fb398f41))
* **cli:** review runs one, three or six sessions, each on its own areas ([#894](https://github.com/noahdgrant/nightshift/issues/894)) ([e15721f](https://github.com/noahdgrant/nightshift/commit/e15721fde41c90e618808f1cfbfcbfaf4b748169))
* **cli:** ship holds the merge lock from the rebase to the merge ([#906](https://github.com/noahdgrant/nightshift/issues/906)) ([290c3f0](https://github.com/noahdgrant/nightshift/commit/290c3f00b983dadaf6d26a0e7a18f1bdbe18ac84))
* **cli:** ship rebases the unit onto main and tests the result before merging ([#903](https://github.com/noahdgrant/nightshift/issues/903)) ([ffbbde5](https://github.com/noahdgrant/nightshift/commit/ffbbde533ca10cf0a659c47914a83e973165c14a))
* **cli:** transcripts older than a setting are deleted, a week by default ([#889](https://github.com/noahdgrant/nightshift/issues/889)) ([00528c0](https://github.com/noahdgrant/nightshift/commit/00528c008531e7a274d40e1009864350230fe05a))
* **cli:** use GitHub's native issue types where the repository has them ([#896](https://github.com/noahdgrant/nightshift/issues/896)) ([35f0a68](https://github.com/noahdgrant/nightshift/commit/35f0a68a516cc9cafb69d36e0ada3d77dc2be003))
* **skills:** fold the principles that answer real mistakes into the state skills ([#897](https://github.com/noahdgrant/nightshift/issues/897)) ([41f5859](https://github.com/noahdgrant/nightshift/commit/41f58598a07ff2ba1ed6291254b326c5ad01794c))


### Bug Fixes

* **cli:** remove a unit's lock file when its run ends ([#880](https://github.com/noahdgrant/nightshift/issues/880)) ([ec9c933](https://github.com/noahdgrant/nightshift/commit/ec9c93327fff385aac3d662d0f2c9787833d4f56))
* **cli:** the test forge for native types implements the new blocker method ([#899](https://github.com/noahdgrant/nightshift/issues/899)) ([62c7f23](https://github.com/noahdgrant/nightshift/commit/62c7f23bf46717a8a6fc8da73551b7599dbb8264))
* **cli:** triage failures count toward the failures-in-a-row stop ([#884](https://github.com/noahdgrant/nightshift/issues/884)) ([76535ad](https://github.com/noahdgrant/nightshift/commit/76535ad8c055be607dd3e2434c377028d27516ff))
* **skills:** ns-review states what it ran and what it only read ([#883](https://github.com/noahdgrant/nightshift/issues/883)) ([7f4ee36](https://github.com/noahdgrant/nightshift/commit/7f4ee363b82dbf05e07509fa507ce3b0507ad218))

## [0.4.0](https://github.com/noahdgrant/nightshift/compare/v0.3.0...v0.4.0) (2026-10-21)


### Features

* **cli:** a verification sees the previous verification and re-checks what failed ([#862](https://github.com/noahdgrant/nightshift/issues/862)) ([5d978f5](https://github.com/noahdgrant/nightshift/commit/5d978f5a192647939c7dca2442e7f1fd61561132))
* **cli:** keep each state's harness session and show it with ns log ([#873](https://github.com/noahdgrant/nightshift/issues/873)) ([31a8465](https://github.com/noahdgrant/nightshift/commit/31a8465c28063961b4741de50b039d80f41bca85))
* **cli:** merge only changes that touch no human_review path ([#872](https://github.com/noahdgrant/nightshift/issues/872)) ([9eee07e](https://github.com/noahdgrant/nightshift/commit/9eee07e0ba5e872269e5ff53d114393758eccdae))
* **cli:** ns assigns the unit and its change request to the account it runs as ([#868](https://github.com/noahdgrant/nightshift/issues/868)) ([87167c5](https://github.com/noahdgrant/nightshift/commit/87167c57fc58c437f5f3a6f934f27ced5dcbb064))
* **cli:** ns auto stops the night after units keep going wrong on the harness ([#861](https://github.com/noahdgrant/nightshift/issues/861)) ([c48e51b](https://github.com/noahdgrant/nightshift/commit/c48e51b9cc3886825f439374a3e521e02d4394e6))
* **cli:** ns eval review measures the review skill on known cases ([#874](https://github.com/noahdgrant/nightshift/issues/874)) ([d2bf77f](https://github.com/noahdgrant/nightshift/commit/d2bf77f71277adee4fcdf2aa9919ccaa8c17fe62))
* **cli:** the attempts a unit gets are a setting in nightshift.toml, three by default ([#878](https://github.com/noahdgrant/nightshift/issues/878)) ([692f39b](https://github.com/noahdgrant/nightshift/commit/692f39be43aad1de672ba0cba396b9b987335ead))


### Bug Fixes

* **cli:** resuming a unit stuck in a check with nothing changed goes back to build ([#863](https://github.com/noahdgrant/nightshift/issues/863)) ([adde79a](https://github.com/noahdgrant/nightshift/commit/adde79ab0849619d6f2fd5dd5f1d3c35b6e24e0a))

## [0.3.0](https://github.com/noahdgrant/nightshift/compare/v0.2.0...v0.3.0) (2026-10-21)


### Features

* **cli:** a review sees the findings of the previous review and says which are fixed ([#856](https://github.com/noahdgrant/nightshift/issues/856)) ([b4c3459](https://github.com/noahdgrant/nightshift/commit/b4c34594e3f7d8547825a18aeb924985ebe84d1c))
* **cli:** ns --version inside a pinned repository shows the version that runs there ([#848](https://github.com/noahdgrant/nightshift/issues/848)) ([a85fd95](https://github.com/noahdgrant/nightshift/commit/a85fd95fd2a2bd88ee87d6bc8fbf357b40d6b6c6))
* **cli:** ns status lists where each unit stands ([#854](https://github.com/noahdgrant/nightshift/issues/854)) ([9b8b219](https://github.com/noahdgrant/nightshift/commit/9b8b2191bf5b3d1d26dcd5809fab1336e62b8b3a))


### Bug Fixes

* **skills:** ns-verify fails a unit only for its requirements, not for a build claim it did not re-run ([#853](https://github.com/noahdgrant/nightshift/issues/853)) ([41401e1](https://github.com/noahdgrant/nightshift/commit/41401e12d1cc50c6a7c22c017d260bdb835e4455))

## [0.2.0](https://github.com/noahdgrant/nightshift/compare/v0.1.0...v0.2.0) (2026-10-21)


### Features

* **cli:** ns doctor checks the setup of a repository ([#840](https://github.com/noahdgrant/nightshift/issues/840)) ([42b5485](https://github.com/noahdgrant/nightshift/commit/42b548510a4207e07bcf5fb740163c439edd8a66))
* **cli:** ship gives the change request the unit's type, area and priority labels ([#844](https://github.com/noahdgrant/nightshift/issues/844)) ([49263aa](https://github.com/noahdgrant/nightshift/commit/49263aa6ff537c01ae333605ebd9a81e591387d1))


### Bug Fixes

* **cli:** clean up a finished unit once its change request has merged ([b527307](https://github.com/noahdgrant/nightshift/commit/b5273078b24bc46a492994d1ff1c3f7420747061))
* **cli:** nothing cleans up a unit after a human merges its change request ([#841](https://github.com/noahdgrant/nightshift/issues/841)) ([b527307](https://github.com/noahdgrant/nightshift/commit/b5273078b24bc46a492994d1ff1c3f7420747061))


### Documentation

* **repo:** add pin and sync to the glossary ([#837](https://github.com/noahdgrant/nightshift/issues/837)) ([4a4ac6d](https://github.com/noahdgrant/nightshift/commit/4a4ac6d1f255f3c561ddcb8a1eed66603d64d7b7))
* **repo:** the README says ns run stops before the first state, which it does not ([#845](https://github.com/noahdgrant/nightshift/issues/845)) ([c2c42bb](https://github.com/noahdgrant/nightshift/commit/c2c42bb57534b0d6df35164f67a577cbce602398))

## 0.1.0 (2026-10-19)


### Refactoring

* **repo:** replace the old nightshift with v2 and pin repositories to an ns version ([#830](https://github.com/noahdgrant/nightshift/issues/830)) ([4593c06](https://github.com/noahdgrant/nightshift/commit/4593c062fa47667fdfca4c97bb9fcd0b70db2b2f))
