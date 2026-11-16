import UIKit

/// An invisible first responder that brings up the system keyboard while the
/// game's virtual keyboard has a field open, and hands what is typed to the
/// game (docs/host-keyboard.md). Shared by the tvOS and iOS apps.
///
/// Plain `UIKeyInput`, not `UITextInput`: the system then never shows marked
/// (composing) text here, and only committed text arrives, as on the Mac.
/// Return arrives as "\n", which the Rust side turns into Done. Hiding the
/// keyboard does not cancel the field; Escape on a hardware keyboard does,
/// like the game's own Back.
final class GameKeyInput: UIView, UIKeyInput {
    var host: OpaquePointer?
    /// Called when a field opens (true) or closes (false).
    var onFieldChange: ((Bool) -> Void)?

    override var canBecomeFirstResponder: Bool { true }

    // Always true, so deleteBackward is sent even though this view keeps no
    // text: the game's field holds it.
    var hasText: Bool { true }

    func insertText(_ text: String) {
        guard let host else { return }
        _ = h2host_text_insert(host, text)
    }

    func deleteBackward() {
        guard let host else { return }
        _ = h2host_text_delete_backward(host)
    }

    // Names, gamertags and clan names: no corrections or smart punctuation.
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no
    // Not asciiCapable: the game has accent pages and filters what it can't draw.
    var keyboardType: UIKeyboardType = .default
    var returnKeyType: UIReturnKeyType = .done
    var keyboardAppearance: UIKeyboardAppearance = .dark

    override var keyCommands: [UIKeyCommand]? {
        let escape = UIKeyCommand(input: UIKeyCommand.inputEscape, modifierFlags: [], action: #selector(cancelField))
        escape.wantsPriorityOverSystemBehavior = true
        return [escape]
    }

    @objc private func cancelField() {
        guard let host else { return }
        _ = h2host_text_cancel(host)
    }

    /// Follows the game's field: shows the keyboard when one opens and hides
    /// it when it closes. Call once per display frame.
    func follow() {
        guard let host else { return }
        switch h2host_text_edge(host) {
        case 1:
            fieldOpen = true
            onFieldChange?(true)
            becomeFirstResponder()
        case -1:
            fieldOpen = false
            resignFirstResponder()
            onFieldChange?(false)
        default:
            break
        }
    }

    /// A field is open (the keyboard may have been dismissed meanwhile).
    private(set) var fieldOpen = false

    /// Brings the keyboard back for an open field after the player hid it.
    @objc func reshow() {
        if fieldOpen && !isFirstResponder {
            becomeFirstResponder()
        }
    }
}
