import XCTest
@testable import Envoix_iOS

final class SendPresentationTests: XCTestCase {
    func testSelectionCatalogProvidesStaticCopy() {
        let cases: [(String, String, String)] = [
            ("common.remove", "Remove", "移除"),
            (
                "send.concurrent.finish_receive",
                "Finish receiving before starting a send.",
                "请先完成接收任务，再开始发送。"
            ),
            (
                "send.invite.receiver_link_detail",
                "Paste the link or QR result from the receiving device.",
                "粘贴接收端生成的链接或二维码内容。"
            ),
            ("send.invite.receiver_link_title", "Receiver invite link", "接收端邀请链接"),
            ("send.path.copied", "Selected paths copied", "已复制所选路径"),
            ("send.path.copy", "Copy Selected Paths", "复制已选路径"),
            ("send.path.copy_help", "Copy selected paths", "复制所选路径"),
            ("send.path.placeholder", "Paste an absolute file path here", "在这里粘贴绝对文件路径"),
            ("send.path.use", "Use Path", "使用路径"),
            ("send.path.use_help", "Use pasted path", "使用粘贴的路径"),
            ("send.selection.clipboard_action", "Paste File or Image", "粘贴文件或图片"),
            ("send.selection.choose", "Choose files or folders", "选择文件或文件夹"),
            (
                "send.selection.guidance.desktop",
                "Choose, drop, or paste files and images. Folder structure is preserved.",
                "可选择、拖入或粘贴文件与图片；目录结构会完整保留。"
            ),
            (
                "send.selection.guidance.mobile",
                "Choose Photos, files, or one or more folders. Folder structure is preserved.",
                "可选择照片、文件或一个或多个文件夹；目录结构会完整保留。"
            ),
            (
                "send.selection.preparing",
                "Reading and validating the selected items…",
                "正在读取并验证所选项目…"
            ),
            ("send.selection.source.files", "Files", "文件"),
            ("send.selection.source.folder", "Folder", "文件夹"),
            ("send.selection.source.photos", "Photos", "照片"),
            ("send.selection.source_access.approve", "Send accessible content", "发送可访问内容"),
            (
                "send.selection.source_access.detail",
                "Some descendants could not be read. Send only accessible content or remove this root.",
                "部分子项目无法读取。你可以仅发送可访问内容，或移除此根项目。"
            ),
            ("send.selection.source_access.title", "Source access decision", "来源访问决定"),
            (
                "send.selection.subtitle.desktop_empty",
                "Drop files or folders here, or click to choose.",
                "把文件或文件夹拖到这里，或点击选择。"
            ),
            (
                "send.selection.subtitle.desktop_file",
                "Ready to send. Click to replace.",
                "已准备发送，点击可替换。"
            ),
            ("send.selection.subtitle.folder", "Folder structure will be preserved.", "将完整保留文件夹结构。"),
            ("send.selection.subtitle.mobile_empty", "Tap to open Files.", "点击打开文件。"),
            ("send.selection.subtitle.mobile_file", "Ready to send.", "已准备发送。"),
            ("send.selection.subtitle.multiple", "These items will be sent together.", "这些项目将作为一批发送。"),
            ("send.selection.title", "Items to send", "要发送的项目"),
        ]
        for (key, english, chinese) in cases {
            XCTAssertEqual(AppText.localized(key, language: "en"), english, key)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese, key)
        }
    }

    func testSelectionProjectionCoversPlatformAndContentState() {
        XCTAssertEqual(title(itemCount: 0), "Choose files or folders")
        XCTAssertEqual(title(itemCount: 1, name: "report.pdf"), "report.pdf")
        XCTAssertEqual(title(itemCount: 2), "2 items selected")
        XCTAssertEqual(
            SendPresentationText.selectionTitle(
                itemCount: 1,
                singleItemName: "照片.jpg",
                language: "zh-Hans"
            ),
            "照片.jpg"
        )

        XCTAssertEqual(subtitle(itemCount: 0, platform: .mobile), "Tap to open Files.")
        XCTAssertEqual(
            subtitle(itemCount: 0, platform: .desktop),
            "Drop files or folders here, or click to choose."
        )
        XCTAssertEqual(subtitle(itemCount: 1, isDirectory: true), "Folder structure will be preserved.")
        XCTAssertEqual(subtitle(itemCount: 1, platform: .desktop), "Ready to send. Click to replace.")
        XCTAssertEqual(subtitle(itemCount: 2), "These items will be sent together.")
    }

    func testInventoryProjectionUsesNativePluralRules() {
        XCTAssertEqual(
            SendPresentationText.inventorySummary(
                rootCount: 1,
                fileCount: 1,
                folderCount: 0,
                warningCount: 0,
                byteDescription: "12 KB",
                language: "en"
            ),
            "1 root · 1 file · 0 folders · 12 KB"
        )
        XCTAssertEqual(
            SendPresentationText.inventorySummary(
                rootCount: 1,
                fileCount: 2,
                folderCount: 0,
                warningCount: 1,
                byteDescription: "12 KB",
                language: "en"
            ),
            "1 root · 2 files · 0 folders · 12 KB · 1 warning"
        )
        XCTAssertEqual(
            SendPresentationText.inventorySummary(
                rootCount: 2,
                fileCount: 3,
                folderCount: 1,
                warningCount: 2,
                byteDescription: "1 MB",
                language: "zh-Hans"
            ),
            "2 个根项目 · 3 个文件 · 1 个文件夹 · 1 MB · 2 个警告"
        )
        XCTAssertEqual(
            SendPresentationText.additionalTopLevelItems(1, language: "en"),
            "1 more top-level item is included."
        )
        XCTAssertEqual(
            SendPresentationText.additionalTopLevelItems(3, language: "zh-Hans"),
            "还包含 3 个顶层项目。"
        )
        XCTAssertEqual(SendPresentationText.removeItem("draft.txt", language: "en"), "Remove draft.txt")
        XCTAssertEqual(SendPresentationText.removeItem("草稿.txt", language: "zh-Hans"), "移除草稿.txt")
    }

    func testPhotoProgressClampsInvalidCounters() {
        XCTAssertEqual(
            SendPresentationText.photoImportProgress(itemNumber: 2, itemCount: 4, language: "en"),
            "Preparing photo 2 of 4…"
        )
        XCTAssertEqual(
            SendPresentationText.photoImportProgress(itemNumber: -1, itemCount: -2, language: "zh-Hans"),
            "正在准备第 0/0 个照片项目…"
        )
    }

    private func title(itemCount: Int, name: String? = nil) -> String {
        SendPresentationText.selectionTitle(
            itemCount: itemCount,
            singleItemName: name,
            language: "en"
        )
    }

    private func subtitle(
        itemCount: Int,
        isDirectory: Bool = false,
        platform: SendPresentationPlatform = .mobile
    ) -> String {
        SendPresentationText.selectionSubtitle(
            itemCount: itemCount,
            singleItemIsDirectory: isDirectory,
            platform: platform,
            language: "en"
        )
    }
}
