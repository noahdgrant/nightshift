# Changelog

## [0.8.0](https://github.com/noahdgrant/nightshift/compare/v0.7.1...v0.8.0) (2026-11-11)


### Features

* **cli:** a grade records the models and token use of the sessions it graded, and ns grades splits by model ([#1038](https://github.com/noahdgrant/nightshift/issues/1038)) ([da68c00](https://github.com/noahdgrant/nightshift/commit/da68c0066bfad93e595900b5a5d2cbdbee458d73))
* **cli:** a runner starts ns unattended on a schedule ([#1041](https://github.com/noahdgrant/nightshift/issues/1041)) ([148491a](https://github.com/noahdgrant/nightshift/commit/148491a3f46b496f60931507599048f715a6b999))
* **cli:** ns eval covers the verify skill ([#1042](https://github.com/noahdgrant/nightshift/issues/1042)) ([e0b7d20](https://github.com/noahdgrant/nightshift/commit/e0b7d20c3923d9fba5e990c3f97d8c1ecc830715))
* **cli:** ns eval replays past units on another model and shows quality against cost ([#1044](https://github.com/noahdgrant/nightshift/issues/1044)) ([81aa8cf](https://github.com/noahdgrant/nightshift/commit/81aa8cf89babfbebdf8859a51f4efaffabdea95a))
* **cli:** ns eval suggests merged units worth keeping as review cases ([#1043](https://github.com/noahdgrant/nightshift/issues/1043)) ([7e73ae5](https://github.com/noahdgrant/nightshift/commit/7e73ae5d9414b84b9702c64761e7a93491a254e8))
* **cli:** ns grades shows cycle time and cost for each unit ([#1039](https://github.com/noahdgrant/nightshift/issues/1039)) ([15891d8](https://github.com/noahdgrant/nightshift/commit/15891d8b6da22f24d2b514866ff1c11ef67ed725))
* **cli:** ns grades shows cycle time and token cost for each week of merged units ([15891d8](https://github.com/noahdgrant/nightshift/commit/15891d8b6da22f24d2b514866ff1c11ef67ed725))
* **cli:** ns grades shows what share of a unit's tokens the retrospective takes ([#1040](https://github.com/noahdgrant/nightshift/issues/1040)) ([5b4fe3e](https://github.com/noahdgrant/nightshift/commit/5b4fe3efa35c7917a435ee36b7dbda273d44a9ac))
* **skills:** a ci-design skill, and ns init writes the title check ([#1046](https://github.com/noahdgrant/nightshift/issues/1046)) ([0948ef2](https://github.com/noahdgrant/nightshift/commit/0948ef269a5c04c83aa16f5b2293e3cb6fa5978d))


### Bug Fixes

* **cli:** a usage limit on the harness is waited out, not retried for seconds ([#1036](https://github.com/noahdgrant/nightshift/issues/1036)) ([b9eeda6](https://github.com/noahdgrant/nightshift/commit/b9eeda64aa77fc368a642f228ae6dbc82c75372d))
* **cli:** the retrospective keeps its grades when a transcript is unreadable, and two small slips ([#1048](https://github.com/noahdgrant/nightshift/issues/1048)) ([56e913e](https://github.com/noahdgrant/nightshift/commit/56e913eee67b3d4624402fafebd9ba24667889f0)), closes [#1047](https://github.com/noahdgrant/nightshift/issues/1047)

## [0.7.1](https://github.com/noahdgrant/nightshift/compare/v0.7.0...v0.7.1) (2026-11-10)


### Bug Fixes

* **cli:** the closes-unit workflow leaves no cancelled run, so requiring its check does not block a merge ([#1032](https://github.com/noahdgrant/nightshift/issues/1032)) ([0c6370f](https://github.com/noahdgrant/nightshift/commit/0c6370f638d5737bb895e0e039952733e74507bc))

## [0.7.0](https://github.com/noahdgrant/nightshift/compare/v0.6.0...v0.7.0) (2026-11-09)


### Features

* **cli:** a repository sets how long ship waits for CI in nightshift.toml ([#1028](https://github.com/noahdgrant/nightshift/issues/1028)) ([0f28097](https://github.com/noahdgrant/nightshift/commit/0f280978522f6d3414ea135400d98f50b6e48774))
* **cli:** a retrospective state after ship grades a merged unit on one rubric ([#973](https://github.com/noahdgrant/nightshift/issues/973)) ([529efd8](https://github.com/noahdgrant/nightshift/commit/529efd81c23214c149fd71e689340ef3577451af))
* **cli:** ns eval add turns a merged change that carried a defect into a review case ([#995](https://github.com/noahdgrant/nightshift/issues/995)) ([1e38824](https://github.com/noahdgrant/nightshift/commit/1e38824af8481c57dd83efae6bc44748104eb34a))
* **cli:** ns grades shows each rubric's pass rate per week and per ns version ([#975](https://github.com/noahdgrant/nightshift/issues/975)) ([e182c53](https://github.com/noahdgrant/nightshift/commit/e182c53e41d6f697064f4682439851ba8e177201))
* **cli:** ns grades shows how long changes wait for a person ([#996](https://github.com/noahdgrant/nightshift/issues/996)) ([38ca556](https://github.com/noahdgrant/nightshift/commit/38ca556b7adcf9629ca925564d43057b0d363bbe))
* **cli:** ns improve files a unit for a cause that three graded units failed on ([#977](https://github.com/noahdgrant/nightshift/issues/977)) ([f957b3e](https://github.com/noahdgrant/nightshift/commit/f957b3e9892344347a7a4df83b963d9f3ad99083))
* **cli:** ns-review reads the repository's own page on what review should look hardest at and leave out ([#992](https://github.com/noahdgrant/nightshift/issues/992)) ([4850cbe](https://github.com/noahdgrant/nightshift/commit/4850cbea7be5f9d996b570c62f77df88a6e0b5bb)), closes [#982](https://github.com/noahdgrant/nightshift/issues/982)
* **cli:** review re-reads only what changed since the last round and flags a whole class of mistake at once ([#991](https://github.com/noahdgrant/nightshift/issues/991)) ([0b3016e](https://github.com/noahdgrant/nightshift/commit/0b3016ecad153202d706683697baac759e93027d))
* **cli:** the retrospective grades four default rubrics and a repository's own ([#974](https://github.com/noahdgrant/nightshift/issues/974)) ([4dff4b3](https://github.com/noahdgrant/nightshift/commit/4dff4b3cc17175842d793eb61e5bb44210d30ca2))
* **skills:** a service-design skill that design, build and review use for a network service ([#980](https://github.com/noahdgrant/nightshift/issues/980)) ([e765e44](https://github.com/noahdgrant/nightshift/commit/e765e44afeb77fb21b4c5f3e658f482aa12c38c8))
* **skills:** build and verify say which instructions they had to work around ([#967](https://github.com/noahdgrant/nightshift/issues/967)) ([ace7d18](https://github.com/noahdgrant/nightshift/commit/ace7d18cd1987075a3278340581ce1eed47efd8f))
* **skills:** build ends its summary with what a person must be able to defend ([#969](https://github.com/noahdgrant/nightshift/issues/969)) ([456c4e3](https://github.com/noahdgrant/nightshift/commit/456c4e3558685929368a00028b979a54ef4f2098))
* **skills:** Defend this in the build summary also says how to undo the change ([#993](https://github.com/noahdgrant/nightshift/issues/993)) ([cc73369](https://github.com/noahdgrant/nightshift/commit/cc7336997309f01bace0c6b4dbfe97d8f3dfd2d6))
* **skills:** ns-whiteboard-defence answers why something was built that way and how it works from the record ([#994](https://github.com/noahdgrant/nightshift/issues/994)) ([bcc834f](https://github.com/noahdgrant/nightshift/commit/bcc834f65fdafcb9e2b252743178bd44c003cd28))
* **skills:** review says which fix level would remove a finding's whole category ([#970](https://github.com/noahdgrant/nightshift/issues/970)) ([a66754e](https://github.com/noahdgrant/nightshift/commit/a66754e52d375c41ca37aeac4e7c85e307979a7b))


### Bug Fixes

* **cli:** ns doctor counts a branch ruleset as protection and checks that it requires the closes-unit check ([#1027](https://github.com/noahdgrant/nightshift/issues/1027)) ([af36888](https://github.com/noahdgrant/nightshift/commit/af36888dbd80dda1ab15d798ee8fba08d56bb884))
* **cli:** ns init sets the description of the labels it owns, including a renamed one ([#956](https://github.com/noahdgrant/nightshift/issues/956)) ([7300fa9](https://github.com/noahdgrant/nightshift/commit/7300fa9d52f1c04051d0179e348ff885a546ed1d))
* **cli:** ns sync keeps the skills it writes through a symlinked skills folder out of the repository's changes ([#1026](https://github.com/noahdgrant/nightshift/issues/1026)) ([706266f](https://github.com/noahdgrant/nightshift/commit/706266f311a377e5c880e89a18f93aa3efe6b56b))
* **cli:** ns upgrade and ns init from a worktree say which checkout they changed ([#968](https://github.com/noahdgrant/nightshift/issues/968)) ([cd79f6a](https://github.com/noahdgrant/nightshift/commit/cd79f6ac9727ea73e5c53df2ed1b6b1d2e5f43a7))
* **cli:** say how to run the built ns against the fake gh, and name GH_TOKEN in the error ([#1005](https://github.com/noahdgrant/nightshift/issues/1005)) ([844df76](https://github.com/noahdgrant/nightshift/commit/844df76e179d83d6791f9e4a53014ade5b41c076))
* **cli:** sessions started under NS_DEV use the ns that started them ([#1021](https://github.com/noahdgrant/nightshift/issues/1021)) ([2bac455](https://github.com/noahdgrant/nightshift/commit/2bac4553e8161d87459c79a664a601a4e5d51472))
* **cli:** ship opens a new change request when the only one for the branch is closed ([#1020](https://github.com/noahdgrant/nightshift/issues/1020)) ([45ab25a](https://github.com/noahdgrant/nightshift/commit/45ab25ad5273266976b1d977bb395c6fac778915))
* **cli:** the closes-unit check skips a release change request by its branch, not by a label ([#988](https://github.com/noahdgrant/nightshift/issues/988)) ([79e64e3](https://github.com/noahdgrant/nightshift/commit/79e64e35f64b142152b40384372a9f7e4e1d7605))
* **skills:** an unproven Should or Could does not stick a unit in verification ([#1019](https://github.com/noahdgrant/nightshift/issues/1019)) ([9f27577](https://github.com/noahdgrant/nightshift/commit/9f275772ddc85d9ed3c07ac711703b8417ba46ba))
* **skills:** verify builds outside the workspace into an output directory outside it ([#972](https://github.com/noahdgrant/nightshift/issues/972)) ([3ef0607](https://github.com/noahdgrant/nightshift/commit/3ef060787b9b7c30f271a8e8624d25adc4b36f53))


### Documentation

* add the nightshift logo ([#978](https://github.com/noahdgrant/nightshift/issues/978)) ([08a37d4](https://github.com/noahdgrant/nightshift/commit/08a37d413fb017bf5db71f650839032069bed243))
* **repo:** add the Lloyd talk on factories that improve themselves to research ([#1004](https://github.com/noahdgrant/nightshift/issues/1004)) ([b078010](https://github.com/noahdgrant/nightshift/commit/b0780108e80b6a8d4b1635e4692f9dbd815ce01a))
* **repo:** keep research sources and a youtube-transcript dev skill ([#990](https://github.com/noahdgrant/nightshift/issues/990)) ([dd344b9](https://github.com/noahdgrant/nightshift/commit/dd344b9290143a30e90cd145634e0311cd2fd6b9))

## [0.6.0](https://github.com/noahdgrant/nightshift/compare/v0.5.0...v0.6.0) (2026-11-08)


### Features

* **cli:** a change inside a human-review region holds the merge for a human ([#944](https://github.com/noahdgrant/nightshift/issues/944)) ([0eb3463](https://github.com/noahdgrant/nightshift/commit/0eb3463b8fd0cc8b70801a82dfa360d3b47521ae))
* **cli:** ns check fails on unbalanced or nested human-review markers ([#943](https://github.com/noahdgrant/nightshift/issues/943)) ([4098754](https://github.com/noahdgrant/nightshift/commit/40987547ea5b8882f49ad6d875e67f39b07e5ece))
* **cli:** ns doctor checks a repository is ready for ns ([#927](https://github.com/noahdgrant/nightshift/issues/927)) ([9db765e](https://github.com/noahdgrant/nightshift/commit/9db765e9c4e12317617be805e337f8c57fe5dd01))
* **cli:** ns init asks for the repository's checks command and writes .nightshift/check ([#940](https://github.com/noahdgrant/nightshift/issues/940)) ([1f69bb1](https://github.com/noahdgrant/nightshift/commit/1f69bb1b9191f1c5b24db4f16dc2e3ce216b85ec))
* **cli:** ns init installs the closes-unit check in a repository ([#925](https://github.com/noahdgrant/nightshift/issues/925)) ([53fe99a](https://github.com/noahdgrant/nightshift/commit/53fe99acc90fcc2db1db1a83a125496a1a02fc41))
* **cli:** ns init scaffolds docs/agents/ for the stack, how to run, and labels ([#941](https://github.com/noahdgrant/nightshift/issues/941)) ([1f15107](https://github.com/noahdgrant/nightshift/commit/1f151073708c12ab61a33111bd7afb556ca26b80))
* **cli:** ns init writes AGENTS.md and CLAUDE.md when the repository has neither ([#928](https://github.com/noahdgrant/nightshift/issues/928)) ([df70683](https://github.com/noahdgrant/nightshift/commit/df7068332d5517b8c910e98268c11ee3686bdbf2))
* **cli:** ns takes over a change request a person opened for a stuck unit ([#952](https://github.com/noahdgrant/nightshift/issues/952)) ([da6789d](https://github.com/noahdgrant/nightshift/commit/da6789d1974e897d0384e3c80c39fcbdb3112446))
* **cli:** the coding standards ship with ns as one document that repositories point to ([#924](https://github.com/noahdgrant/nightshift/issues/924)) ([a2c0449](https://github.com/noahdgrant/nightshift/commit/a2c04491ce5cf1481294290964cee4c86fcc40ba))
* **cli:** the merge policy starts closed in a repository that has not chosen one ([#923](https://github.com/noahdgrant/nightshift/issues/923)) ([2fa2ed3](https://github.com/noahdgrant/nightshift/commit/2fa2ed31c67de9636270105d90c69482f13eb291))
* **cli:** the review is given the repository's coding standards ([#937](https://github.com/noahdgrant/nightshift/issues/937)) ([c413e8d](https://github.com/noahdgrant/nightshift/commit/c413e8d61f595cd009a7f7653865ad9f3407d5cd))
* **cli:** the shipped coding standards are one committed file, kept current by ns sync ([#936](https://github.com/noahdgrant/nightshift/issues/936)) ([8f41e76](https://github.com/noahdgrant/nightshift/commit/8f41e76195ca43a57d240150669248316d96aa01))
* **skills:** review checks that what a committed file names exists in a fresh checkout ([#934](https://github.com/noahdgrant/nightshift/issues/934)) ([53bf941](https://github.com/noahdgrant/nightshift/commit/53bf94109b022a5e9d0ab2ae6a8eccfa142f84f8))


### Bug Fixes

* **cli:** a build that leaves two commits is squashed by the build agent and does not cost an attempt ([#949](https://github.com/noahdgrant/nightshift/issues/949)) ([379f250](https://github.com/noahdgrant/nightshift/commit/379f25056ba48e31355546231a3b3b2c6d302f1e))
* **cli:** a unit refreshed onto a moved base is judged against the base it now sits on ([#935](https://github.com/noahdgrant/nightshift/issues/935)) ([e9a0c5e](https://github.com/noahdgrant/nightshift/commit/e9a0c5ef1817b094147eaaef873f861d6f6bd4b5))
* **cli:** an empty list of units from the forge is asked for again ([#909](https://github.com/noahdgrant/nightshift/issues/909)) ([870afec](https://github.com/noahdgrant/nightshift/commit/870afec3209c28f33477bb01bd769e5d00737276))
* **skills:** the define step shows the spec in the conversation and asks for approval ([3c12f30](https://github.com/noahdgrant/nightshift/commit/3c12f3059747d84d343f65d7e79487b2be87344d))
* **skills:** the define step tells the session to write the spec to a file, not to the unit ([#951](https://github.com/noahdgrant/nightshift/issues/951)) ([3c12f30](https://github.com/noahdgrant/nightshift/commit/3c12f3059747d84d343f65d7e79487b2be87344d))


### Refactoring

* **cli:** a unit waiting for a human merge is told apart from a stuck unit ([#950](https://github.com/noahdgrant/nightshift/issues/950)) ([2e3fe89](https://github.com/noahdgrant/nightshift/commit/2e3fe89a0dde0bc386779a71a6dca83de4602fed))
* **cli:** the closes-unit workflow ns init writes is the one this repository runs ([#942](https://github.com/noahdgrant/nightshift/issues/942)) ([ef76ddc](https://github.com/noahdgrant/nightshift/commit/ef76ddcfdfa215871d32593688c4fb2b42821d94))

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
