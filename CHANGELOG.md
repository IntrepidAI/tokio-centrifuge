# Changelog

## 0.3.0 - WIP

### Breaking changes

Added `config` parameter to `new_subscription` method.

Migration:

Replace `client.new_subscription(channel)` with `client.new_subscription(channel, Default::default())`.

### Added

 - (client) added support for subscription tokens
 - (client) `data` field in `Config` and `SubscriptionConfig`

## 0.0.0 - 0.2.6 (2024-05-03 - 2025-09-29)

Initial commits. Software was not stabilized yet, so changes weren't documented at that point.
