package dev.envoix.app

internal data class HostedNdefSnapshot(
    val generation: Long,
    val message: ByteArray,
)

/**
 * Minimal read-only NFC Forum Type 4 Tag state machine.
 *
 * It exposes one NDEF message only for the lifetime of an ISO-DEP activation.
 * The generation callback makes a hide/background action invalidate an
 * in-flight read before any further invitation bytes are returned.
 */
internal class NfcType4TagProtocol(
    private val snapshot: () -> HostedNdefSnapshot?,
    private val isCurrent: (Long) -> Boolean,
    private val onMessageRead: (Long) -> Unit = {},
) {
    private var selectedFile = SelectedFile.None
    private var generation: Long? = null
    private var ndefFile: ByteArray? = null
    private var readCompletionReported = false
    private var contiguousNdefReadEnd = 0

    @Synchronized
    fun process(command: ByteArray): ByteArray {
        if (command.size < ISO7816_HEADER_BYTES) return status(WRONG_LENGTH)
        if (command[0] != ISO7816_CLASS) return status(CLASS_NOT_SUPPORTED)
        return when (unsigned(command[1])) {
            SELECT -> select(command)
            READ_BINARY -> readBinary(command)
            else -> status(INSTRUCTION_NOT_SUPPORTED)
        }
    }

    @Synchronized
    fun deactivate() {
        generation = null
        selectedFile = SelectedFile.None
        readCompletionReported = false
        contiguousNdefReadEnd = 0
        ndefFile?.fill(0)
        ndefFile = null
    }

    private fun select(command: ByteArray): ByteArray {
        val data = shortCommandData(command) ?: return status(WRONG_LENGTH)
        val p1 = command[2]
        val p2 = command[3]
        return when {
            p1 == SELECT_BY_NAME &&
                p2 == FIRST_OR_ONLY -> {
                if (isSupportedApplicationAid(data)) {
                    selectApplication()
                } else {
                    deactivate()
                    status(FILE_NOT_FOUND)
                }
            }
            p1 == SELECT_FILE &&
                p2 == NO_RESPONSE_DATA &&
                data.size == FILE_ID_BYTES ->
                selectFile(data)
            else -> {
                if (p1 == SELECT_BY_NAME) deactivate()
                status(INCORRECT_PARAMETERS)
            }
        }
    }

    private fun selectApplication(): ByteArray {
        deactivate()
        val hosted = snapshot() ?: return status(FILE_NOT_FOUND)
        if (hosted.message.isEmpty() ||
            hosted.message.size > MAX_NDEF_MESSAGE_BYTES ||
            !isCurrent(hosted.generation)
        ) {
            hosted.message.fill(0)
            return status(CONDITIONS_NOT_SATISFIED)
        }

        val file = ByteArray(NDEF_LENGTH_BYTES + hosted.message.size)
        file[0] = (hosted.message.size ushr 8).toByte()
        file[1] = hosted.message.size.toByte()
        hosted.message.copyInto(file, destinationOffset = NDEF_LENGTH_BYTES)
        hosted.message.fill(0)
        generation = hosted.generation
        ndefFile = file
        selectedFile = SelectedFile.None
        return status(SUCCESS)
    }

    private fun selectFile(fileId: ByteArray): ByteArray {
        if (!sessionIsCurrent()) return status(CONDITIONS_NOT_SATISFIED)
        selectedFile =
            when {
                fileId.contentEquals(CAPABILITY_CONTAINER_FILE_ID) ->
                    SelectedFile.CapabilityContainer
                fileId.contentEquals(NDEF_FILE_ID) -> SelectedFile.Ndef
                else -> {
                    selectedFile = SelectedFile.None
                    return status(FILE_NOT_FOUND)
                }
            }
        if (selectedFile == SelectedFile.Ndef) contiguousNdefReadEnd = 0
        return status(SUCCESS)
    }

    private fun readBinary(command: ByteArray): ByteArray {
        if (command.size != READ_BINARY_COMMAND_BYTES) return status(WRONG_LENGTH)
        if (!sessionIsCurrent()) return status(CONDITIONS_NOT_SATISFIED)
        val file =
            when (selectedFile) {
                SelectedFile.CapabilityContainer -> CAPABILITY_CONTAINER
                SelectedFile.Ndef -> ndefFile ?: return status(CONDITIONS_NOT_SATISFIED)
                SelectedFile.None -> return status(CONDITIONS_NOT_SATISFIED)
            }
        val offset = unsigned(command[2]) shl 8 or unsigned(command[3])
        if (offset >= file.size) return status(WRONG_PARAMETERS)
        val requested = unsigned(command[4]).let { if (it == 0) 256 else it }
        val end = (offset + requested).coerceAtMost(file.size)
        val response = file.copyOfRange(offset, end) + SUCCESS
        if (selectedFile == SelectedFile.Ndef) {
            if (offset <= contiguousNdefReadEnd) {
                contiguousNdefReadEnd = maxOf(contiguousNdefReadEnd, end)
            }
            val activeGeneration = generation
            if (contiguousNdefReadEnd == file.size &&
                activeGeneration != null &&
                !readCompletionReported
            ) {
                readCompletionReported = true
                onMessageRead(activeGeneration)
            }
        }
        return response
    }

    private fun sessionIsCurrent(): Boolean {
        val activeGeneration = generation ?: return false
        if (isCurrent(activeGeneration)) return true
        deactivate()
        return false
    }

    private fun shortCommandData(command: ByteArray): ByteArray? {
        if (command.size < ISO7816_HEADER_BYTES + 1) return null
        val length = unsigned(command[4])
        val withoutLe = ISO7816_HEADER_BYTES + 1 + length
        if (command.size != withoutLe && command.size != withoutLe + 1) return null
        return command.copyOfRange(ISO7816_HEADER_BYTES + 1, withoutLe)
    }

    private fun status(value: ByteArray): ByteArray = value.copyOf()

    private fun unsigned(byte: Byte): Int = byte.toInt() and 0xff

    private enum class SelectedFile {
        None,
        CapabilityContainer,
        Ndef,
    }

    internal companion object {
        const val MAX_NDEF_FILE_BYTES = 0x7fff
        const val MAX_NDEF_MESSAGE_BYTES = MAX_NDEF_FILE_BYTES - 2

        /**
         * Returns only the APDU's protocol shape for debug diagnostics.
         *
         * The command data, offsets, expected lengths, and response body are
         * deliberately omitted: an HCE trace must never copy invitation bytes
         * (or values that reveal their length) into logcat.
         */
        fun traceCommandShape(command: ByteArray): String {
            if (command.size < ISO7816_HEADER_BYTES) {
                return "malformed-header bytes=${command.size}"
            }
            if (command[0] != ISO7816_CLASS) {
                return "unsupported-class bytes=${command.size}"
            }
            return when (unsignedByte(command[1])) {
                SELECT -> traceSelectShape(command)
                READ_BINARY ->
                    when (command.size) {
                        READ_BINARY_COMMAND_BYTES -> "read-binary-short"
                        else -> "read-binary-unsupported bytes=${command.size}"
                    }
                else -> "unsupported-instruction"
            }
        }

        fun traceResponseStatus(response: ByteArray): String {
            if (response.size < STATUS_WORD_BYTES) return "malformed-response"
            val sw1 = unsignedByte(response[response.lastIndex - 1]).toString(16).padStart(2, '0')
            val sw2 = unsignedByte(response[response.lastIndex]).toString(16).padStart(2, '0')
            return "sw=$sw1$sw2"
        }

        private const val ISO7816_HEADER_BYTES = 4
        private const val READ_BINARY_COMMAND_BYTES = 5
        private const val FILE_ID_BYTES = 2
        private const val NDEF_LENGTH_BYTES = 2
        private const val STATUS_WORD_BYTES = 2

        private const val ISO7816_CLASS: Byte = 0x00
        private const val SELECT = 0xa4
        private const val READ_BINARY = 0xb0
        private const val SELECT_BY_NAME: Byte = 0x04
        private const val SELECT_FILE: Byte = 0x00
        private const val FIRST_OR_ONLY: Byte = 0x00
        private const val NO_RESPONSE_DATA: Byte = 0x0c

        val NDEF_APPLICATION_AID =
            byteArrayOf(0xd2.toByte(), 0x76, 0x00, 0x00, 0x85.toByte(), 0x01, 0x01)
        val ENVOIX_APPLICATION_AID =
            byteArrayOf(0xf0.toByte(), 0x45, 0x4e, 0x56, 0x4f, 0x49, 0x58, 0x01)
        val CAPABILITY_CONTAINER_FILE_ID = byteArrayOf(0xe1.toByte(), 0x03)
        val NDEF_FILE_ID = byteArrayOf(0xe1.toByte(), 0x04)

        private val CAPABILITY_CONTAINER =
            byteArrayOf(
                0x00,
                0x0f,
                0x20,
                0x00,
                0xff.toByte(),
                0x00,
                0xff.toByte(),
                0x04,
                0x06,
                NDEF_FILE_ID[0],
                NDEF_FILE_ID[1],
                (MAX_NDEF_FILE_BYTES ushr 8).toByte(),
                MAX_NDEF_FILE_BYTES.toByte(),
                0x00,
                0xff.toByte(),
            )

        val SUCCESS = byteArrayOf(0x90.toByte(), 0x00)
        val WRONG_LENGTH = byteArrayOf(0x67, 0x00)
        val CONDITIONS_NOT_SATISFIED = byteArrayOf(0x69, 0x85.toByte())
        val INCORRECT_PARAMETERS = byteArrayOf(0x6a, 0x86.toByte())
        val FILE_NOT_FOUND = byteArrayOf(0x6a, 0x82.toByte())
        val WRONG_PARAMETERS = byteArrayOf(0x6b, 0x00)
        val INSTRUCTION_NOT_SUPPORTED = byteArrayOf(0x6d, 0x00)
        val CLASS_NOT_SUPPORTED = byteArrayOf(0x6e, 0x00)

        private fun traceSelectShape(command: ByteArray): String {
            val data =
                traceShortCommandData(command)
                    ?: return "select-malformed bytes=${command.size}"
            return when {
                command[2] == SELECT_BY_NAME &&
                    command[3] == FIRST_OR_ONLY &&
                    data.contentEquals(NDEF_APPLICATION_AID) ->
                    "select-ndef-application"
                command[2] == SELECT_BY_NAME &&
                    command[3] == FIRST_OR_ONLY &&
                    data.contentEquals(ENVOIX_APPLICATION_AID) ->
                    "select-envoix-application"
                command[2] == SELECT_FILE &&
                    command[3] == NO_RESPONSE_DATA &&
                    data.contentEquals(CAPABILITY_CONTAINER_FILE_ID) ->
                    "select-capability-container"
                command[2] == SELECT_FILE &&
                    command[3] == NO_RESPONSE_DATA &&
                    data.contentEquals(NDEF_FILE_ID) ->
                    "select-ndef-file"
                command[2] == SELECT_BY_NAME -> "select-other-application"
                else -> "select-other-file-or-form"
            }
        }

        private fun traceShortCommandData(command: ByteArray): ByteArray? {
            if (command.size < ISO7816_HEADER_BYTES + 1) return null
            val length = unsignedByte(command[4])
            val withoutLe = ISO7816_HEADER_BYTES + 1 + length
            if (command.size != withoutLe && command.size != withoutLe + 1) return null
            return command.copyOfRange(ISO7816_HEADER_BYTES + 1, withoutLe)
        }

        private fun unsignedByte(byte: Byte): Int = byte.toInt() and 0xff

        private fun isSupportedApplicationAid(aid: ByteArray): Boolean =
            aid.contentEquals(NDEF_APPLICATION_AID) ||
                aid.contentEquals(ENVOIX_APPLICATION_AID)
    }
}
