#include <cstdint>
#include <iomanip>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/instance.h>
#include <fcitx/surroundingtext.h>
#include <fcitx-utils/key.h>

extern "C" void skk_zenz_log_key(
    const char *uuid, const char *key, bool release);
extern "C" void skk_zenz_handle_unconsumed_key(
    const char *uuid, bool release, bool isBackspace, bool ignore,
    const char *append);
extern "C" void skk_zenz_handle_commit(
    const char *uuid, const char *text, bool hasCursor,
    std::uint32_t cursor);
extern "C" void skk_zenz_handle_surrounding_text(
    const char *uuid, bool valid, const char *text, std::uint32_t cursor,
    std::uint32_t anchor);

namespace {

std::string uuidString(const fcitx::InputContext *inputContext) {
    std::ostringstream stream;
    stream << std::hex << std::setfill('0');
    for (const auto byte : inputContext->uuid()) {
        stream << std::setw(2) << static_cast<unsigned int>(byte);
    }
    return stream.str();
}

void handleSurroundingText(fcitx::InputContext *inputContext) {
    const auto uuid = uuidString(inputContext);
    const auto &surroundingText = inputContext->surroundingText();
    skk_zenz_handle_surrounding_text(
        uuid.c_str(), surroundingText.isValid(),
        surroundingText.text().c_str(), surroundingText.cursor(),
        surroundingText.anchor());
}

void logKeyEvent(fcitx::KeyEvent &event) {
    const auto uuid = uuidString(event.inputContext());
    const auto keyName = event.key().toString();
    skk_zenz_log_key(
        uuid.c_str(), keyName.c_str(), event.isRelease());
}

void handleUnconsumedKeyEvent(fcitx::KeyEvent &event) {
    if (event.accepted()) {
        return;
    }

    const auto &key = event.key();
    const auto uuid = uuidString(event.inputContext());
    const auto ignore =
        key.sym() == FcitxKey_Shift_L || key.sym() == FcitxKey_Shift_R ||
        key.sym() == FcitxKey_Control_L ||
        key.sym() == FcitxKey_Control_R;

    // A normalized printable key has no remaining modifier. Rust performs
    // the final one-character/control-character validation.
    const auto append =
        key.hasModifier() ? std::string{} :
                            fcitx::Key::keySymToUTF8(key.sym());
    skk_zenz_handle_unconsumed_key(
        uuid.c_str(), event.isRelease(),
        key.sym() == FcitxKey_BackSpace, ignore, append.c_str());
}

class SkkZenzEngine final : public fcitx::InputMethodEngineV2 {
public:
    explicit SkkZenzEngine(fcitx::Instance *instance) {
        eventHandlers_.emplace_back(instance->watchEvent(
            fcitx::EventType::InputContextKeyEvent,
            fcitx::EventWatcherPhase::PreInputMethod,
            [](fcitx::Event &event) {
                logKeyEvent(static_cast<fcitx::KeyEvent &>(event));
            }));

        eventHandlers_.emplace_back(instance->watchEvent(
            fcitx::EventType::InputContextKeyEvent,
            fcitx::EventWatcherPhase::PostInputMethod,
            [](fcitx::Event &event) {
                handleUnconsumedKeyEvent(
                    static_cast<fcitx::KeyEvent &>(event));
            }));

        eventHandlers_.emplace_back(instance->watchEvent(
            fcitx::EventType::InputContextCommitString,
            fcitx::EventWatcherPhase::Default, [](fcitx::Event &event) {
                auto &commitEvent =
                    static_cast<fcitx::CommitStringEvent &>(event);
                const auto uuid =
                    uuidString(commitEvent.inputContext());
                skk_zenz_handle_commit(
                    uuid.c_str(), commitEvent.text().c_str(), false, 0);
            }));

        eventHandlers_.emplace_back(instance->watchEvent(
            fcitx::EventType::InputContextCommitStringWithCursor,
            fcitx::EventWatcherPhase::Default, [](fcitx::Event &event) {
                auto &commitEvent =
                    static_cast<fcitx::CommitStringWithCursorEvent &>(event);
                const auto uuid =
                    uuidString(commitEvent.inputContext());
                skk_zenz_handle_commit(
                    uuid.c_str(), commitEvent.text().c_str(), true,
                    static_cast<std::uint32_t>(commitEvent.cursor()));
            }));

        eventHandlers_.emplace_back(instance->watchEvent(
            fcitx::EventType::InputContextSurroundingTextUpdated,
            fcitx::EventWatcherPhase::Default, [](fcitx::Event &event) {
                auto &inputContextEvent =
                    static_cast<fcitx::InputContextEvent &>(event);
                handleSurroundingText(inputContextEvent.inputContext());
            }));
    }

    void keyEvent(const fcitx::InputMethodEntry &entry,
                  fcitx::KeyEvent &event) override {
        FCITX_UNUSED(entry);

        if (!event.isRelease()) {
            // Workaround: some Wayland clients only publish usable
            // surrounding text after an IM sends a commit_string request.
            // An empty string leaves the document unchanged.
            event.inputContext()->commitString("");
        }

        // Observe only: let every key continue to the application.
    }

private:
    std::vector<std::unique_ptr<
        fcitx::HandlerTableEntry<fcitx::EventHandler>>>
        eventHandlers_;
};

class SkkZenzEngineFactory final : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new SkkZenzEngine(manager->instance());
    }
};

} // namespace

FCITX_ADDON_FACTORY(SkkZenzEngineFactory);
