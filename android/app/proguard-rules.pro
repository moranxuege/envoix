# The Rust core (libenvoix_ffi.so) binds to these Java symbols by name via JNI,
# so R8 must not rename or strip them:
#  - `Native` + its native methods → the `Java_dev_envoix_app_Native_*` C symbols
#  - `EventCallback.onEvent` / `LogCallback.log` → looked up with GetMethodID from Rust
-keep class dev.envoix.app.Native { *; }
-keep class dev.envoix.app.EventCallback { *; }
-keep class dev.envoix.app.ManifestV2Callback { *; }
-keep class dev.envoix.app.NearbyInviteCallback { *; }
-keep class dev.envoix.app.LogCallback { *; }
-keepclassmembers class * implements dev.envoix.app.EventCallback { *; }
-keepclassmembers class * implements dev.envoix.app.ManifestV2Callback { *; }
-keepclassmembers class * implements dev.envoix.app.NearbyInviteCallback { *; }
-keepclassmembers class * implements dev.envoix.app.LogCallback { *; }
