# Changelog

## [0.2.0](https://github.com/udondan/fritz/compare/fritz-core-v0.1.1...fritz-core-v0.2.0) (2026-11-24)


### Features

* apply the user of the profile only to its own host ([#57](https://github.com/udondan/fritz/issues/57)) ([406449a](https://github.com/udondan/fritz/commit/406449ad65b3b27851424a2663386cf847d4fc8e))
* cache device service descriptions per firmware ([#117](https://github.com/udondan/fritz/issues/117)) ([a446342](https://github.com/udondan/fritz/commit/a44634244ebd4a1ebeca1de32b5e173171e550ef))
* **fritz-sim:** add a fixture of the FRITZ!WLAN Repeater 1750E ([#58](https://github.com/udondan/fritz/issues/58)) ([64cd15a](https://github.com/udondan/fritz/commit/64cd15aa8c87c6da208e5bc1072871a3bf8ca95d))
* **fritz:** check the whole setup with fritz doctor ([#143](https://github.com/udondan/fritz/issues/143)) ([ec74c55](https://github.com/udondan/fritz/commit/ec74c55e419ea9bbe2d230e97c687566b8e7915f))
* **fritz:** diagnose a host end to end with fritz diagnose ([#142](https://github.com/udondan/fritz/issues/142)) ([a37635d](https://github.com/udondan/fritz/commit/a37635de3fb7c7d5223cdb05c4280eb933f87742))
* **fritz:** diagnose name resolution with fritz dns ([#145](https://github.com/udondan/fritz/issues/145)) ([d47dc1d](https://github.com/udondan/fritz/commit/d47dc1d19dedafa21bff0db2ca366163d4c9669e))
* **fritz:** filter calls and the event log by kind, time, text and category ([#139](https://github.com/udondan/fritz/issues/139)) ([aaf1b1e](https://github.com/udondan/fritz/commit/aaf1b1e93e9b7e46fb7f58b6f53a2a92f13b398f))
* **fritz:** find routers and repeaters on the local network with fritz discover ([#144](https://github.com/udondan/fritz/issues/144)) ([c771da8](https://github.com/udondan/fritz/commit/c771da8208ecfd3e43f125f1ea6358e63e69ac36))
* **fritz:** list powerline adapters with fritz powerline ([#134](https://github.com/udondan/fritz/issues/134)) ([7f37c6b](https://github.com/udondan/fritz/commit/7f37c6bcc015005be2ec7481a0b9c6c9511dd6df))
* **fritz:** list the users of the device and its login settings with fritz users ([#135](https://github.com/udondan/fritz/issues/135)) ([52eac7b](https://github.com/udondan/fritz/commit/52eac7b73fcb9ed7ae408c74bbf17ad41c735252))
* **fritz:** look up one host by name, MAC or IP ([#118](https://github.com/udondan/fritz/issues/118)) ([8abc7b9](https://github.com/udondan/fritz/commit/8abc7b90f600c767ba7a2057cb17d476b54956a7))
* **fritz:** read devices with only a password, like FRITZ!Repeaters ([#56](https://github.com/udondan/fritz/issues/56)) ([0c07820](https://github.com/udondan/fritz/commit/0c078200ec0545a7d0351a3743a082beae25c776))
* **fritz:** read the password from a command ([#112](https://github.com/udondan/fritz/issues/112)) ([f9dc1a1](https://github.com/udondan/fritz/commit/f9dc1a16e7af95c208e65262bc19b5369f1ead6a))
* **fritz:** reboot the device with fritz reboot and wait until it is back ([#136](https://github.com/udondan/fritz/issues/136)) ([efbce56](https://github.com/udondan/fritz/commit/efbce5607d2766cf32527b7cd967bdc425398140))
* **fritz:** report side effects of changes ([#60](https://github.com/udondan/fritz/issues/60)) ([ccdc02d](https://github.com/udondan/fritz/commit/ccdc02da95686eb2175b2855c351d814e81a1887))
* **fritz:** show a QR code to join a wireless network ([#131](https://github.com/udondan/fritz/issues/131)) ([0823259](https://github.com/udondan/fritz/commit/08232597adbbf313180731418e502e7d74352fbd))
* **fritz:** show CPU load, temperature and memory of the device ([#129](https://github.com/udondan/fritz/issues/129)) ([867c35e](https://github.com/udondan/fritz/commit/867c35e28ae6426b72a84c8dd3ed2dd618557d6c))
* **fritz:** show DSL error counters and the line diagnosis ([#133](https://github.com/udondan/fritz/issues/133)) ([7a73ae1](https://github.com/udondan/fritz/commit/7a73ae17b4c4fbb169ff64c9ac7261a5c88cd90d))
* **fritz:** show the clients of each wireless network and mark the guest network ([#125](https://github.com/udondan/fritz/issues/125)) ([fa76825](https://github.com/udondan/fritz/commit/fa768256b4f66ba0da2fb3a072ddedb3cac6f9f2))
* **fritz:** show the counters of LAN interfaces and wireless networks ([#127](https://github.com/udondan/fritz/issues/127)) ([e83b518](https://github.com/udondan/fritz/commit/e83b5186b63dfcb564afa0bbcc884cccfc3fd192))
* **fritz:** show the firmware state with fritz firmware ([#132](https://github.com/udondan/fritz/issues/132)) ([1ab231c](https://github.com/udondan/fritz/commit/1ab231cf6d910037d49f5848e8681ce195841f11))
* **fritz:** show the history of Smart Home devices ([#137](https://github.com/udondan/fritz/issues/137)) ([c4886e4](https://github.com/udondan/fritz/commit/c4886e450ceba7a3ba62d1c2a6fa0ecc9576a237))
* **fritz:** show the IP addresses in the mesh list and tree ([#120](https://github.com/udondan/fritz/issues/120)) ([cd4f590](https://github.com/udondan/fritz/commit/cd4f590f8b954a6f1fc32f044f00294e3c48fdf3))
* **fritz:** show the live WAN traffic with fritz traffic ([#140](https://github.com/udondan/fritz/issues/140)) ([a3d7786](https://github.com/udondan/fritz/commit/a3d77865f13e68d4d5a38469616d15f7cdaf8a04))
* **fritz:** show the mesh as a tree with fritz mesh --tree ([#66](https://github.com/udondan/fritz/issues/66)) ([aab29f6](https://github.com/udondan/fritz/commit/aab29f608c2d92437db9bc9604b7e27f4f72e303))
* **fritz:** show the mesh steering in fritz settings ([#69](https://github.com/udondan/fritz/issues/69)) ([b2eb3b6](https://github.com/udondan/fritz/commit/b2eb3b622838db9a86eeeba71af4cd87cb2988ab))
* **fritz:** show the security settings and details of each wireless network ([#128](https://github.com/udondan/fritz/issues/128)) ([3302b08](https://github.com/udondan/fritz/commit/3302b08437e34969a4de4eca5b5ba0e1792ddd2c))
* **fritz:** show what is reachable from the internet with fritz exposure ([#147](https://github.com/udondan/fritz/issues/147)) ([006a4b7](https://github.com/udondan/fritz/commit/006a4b7a085fd72933adb9137756e3d1d6c7529a))
* **fritz:** watch calls live with fritz calls --watch ([#141](https://github.com/udondan/fritz/issues/141)) ([48dbefd](https://github.com/udondan/fritz/commit/48dbefd471155b2ff1722b2630212cbb20ae1f03))
* log requests for debugging, with secrets hidden ([#116](https://github.com/udondan/fritz/issues/116)) ([8582f4b](https://github.com/udondan/fritz/commit/8582f4bb0ebd756eb271d4027e87fbd29a5d5ff7))


### Bug Fixes

* keep the configuration in ~/.config on macOS as well ([#63](https://github.com/udondan/fritz/issues/63)) ([b27003c](https://github.com/udondan/fritz/commit/b27003c6e651c0c4f2515fdf9118bdab6ad0f84a))


### Documentation

* add a README to fritz-core and use absolute URLs in the README ([#53](https://github.com/udondan/fritz/issues/53)) ([a554d98](https://github.com/udondan/fritz/commit/a554d98719339c190533ba17a7becf818f8edf2f))

## [0.1.1](https://github.com/udondan/fritz/compare/fritz-core-v0.1.0...fritz-core-v0.1.1) (2026-11-21)


### Miscellaneous Chores

* **fritz-core:** Synchronize fritz versions
