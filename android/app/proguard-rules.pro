# Android context bootstrap is the sole hand-written JNI symbol, so R8 must not
# rename or strip `Native.initContext`.
-keep class dev.envoix.app.Native { *; }
