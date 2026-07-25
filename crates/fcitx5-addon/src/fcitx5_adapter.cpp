#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>
#include <fcitx/candidatelist.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/surroundingtext.h>
#include <fcitx/text.h>
#include <fcitx-utils/key.h>

extern "C" {
struct SkkZenzBackend;

struct SkkZenzCandidate {
    const char *text;
    const char *annotation;
};

struct SkkZenzUpdate {
    bool consumed;
    const char *preedit;
    const char *commit;
    const SkkZenzCandidate *candidates;
    std::size_t candidateCount;
    std::size_t selectedCandidate;
};

using SkkZenzUpdateCallback =
    void (*)(void *userData, const SkkZenzUpdate *update);

SkkZenzBackend *skk_zenz_backend_new(const char *configPath);
void skk_zenz_backend_free(SkkZenzBackend *backend);
bool skk_zenz_backend_key(
    SkkZenzBackend *backend, const char *text, const char *key,
    std::uint16_t modifiers, void *userData, SkkZenzUpdateCallback callback);
bool skk_zenz_backend_set_context(
    SkkZenzBackend *backend, const char *context, void *userData,
    SkkZenzUpdateCallback callback);
bool skk_zenz_backend_focus_out(
    SkkZenzBackend *backend, void *userData, SkkZenzUpdateCallback callback);
bool skk_zenz_backend_reset(
    SkkZenzBackend *backend, void *userData, SkkZenzUpdateCallback callback);
bool skk_zenz_backend_select_candidate(
    SkkZenzBackend *backend, std::size_t index, void *userData,
    SkkZenzUpdateCallback callback);
}

namespace {

class CallbackCandidate final : public fcitx::CandidateWord {
public:
    CallbackCandidate(
        fcitx::Text text, fcitx::Text comment,
        std::function<void(fcitx::InputContext *)> callback)
        : CandidateWord(std::move(text)), callback_(std::move(callback)) {
        setComment(std::move(comment));
    }

    void select(fcitx::InputContext *inputContext) const override {
        callback_(inputContext);
    }

private:
    std::function<void(fcitx::InputContext *)> callback_;
};

struct UpdateTarget {
    class SkkZenzEngine *engine;
    fcitx::InputContext *inputContext;
    fcitx::KeyEvent *keyEvent;
};

std::uint16_t backendModifiers(fcitx::KeyStates states) {
    std::uint16_t result = 0;
    if (states.test(fcitx::KeyState::Shift)) {
        result |= 1U << 0;
    }
    if (states.test(fcitx::KeyState::Ctrl)) {
        result |= 1U << 1;
    }
    if (states.test(fcitx::KeyState::Alt)) {
        result |= 1U << 2;
    }
    if (states.test(fcitx::KeyState::Meta)) {
        result |= 1U << 3;
    }
    if (states.test(fcitx::KeyState::Super) ||
        states.test(fcitx::KeyState::Super2)) {
        result |= 1U << 4;
    }
    if (states.test(fcitx::KeyState::Hyper) ||
        states.test(fcitx::KeyState::Hyper2)) {
        result |= 1U << 5;
    }
    if (states.test(fcitx::KeyState::Mod5)) {
        result |= 1U << 6;
    }
    return result;
}

std::string logicalKeyName(const fcitx::Key &key) {
    auto name = fcitx::Key::keySymToUTF8(key.sym());
    if (name.size() == 1 && name[0] >= 'A' && name[0] <= 'Z') {
        name[0] += 'a' - 'A';
        return name;
    }
    if (!name.empty() &&
        static_cast<unsigned char>(name.front()) >= 0x20 &&
        name.front() != '\x7f') {
        return name;
    }
    return fcitx::Key(key.sym()).toString();
}

std::string inputText(const fcitx::Key &key) {
    const auto states = key.states();
    if (states.test(fcitx::KeyState::Ctrl) ||
        states.test(fcitx::KeyState::Alt) ||
        states.test(fcitx::KeyState::Meta) ||
        states.test(fcitx::KeyState::Super) ||
        states.test(fcitx::KeyState::Super2) ||
        states.test(fcitx::KeyState::Hyper) ||
        states.test(fcitx::KeyState::Hyper2)) {
        return {};
    }
    auto text = fcitx::Key::keySymToUTF8(key.sym());
    if (!text.empty() &&
        (static_cast<unsigned char>(text.front()) < 0x20 ||
         text.front() == '\x7f')) {
        return {};
    }
    return text;
}

class SkkZenzEngine final : public fcitx::InputMethodEngineV2 {
public:
    explicit SkkZenzEngine(fcitx::Instance *instance)
        : backend_(skk_zenz_backend_new("skk-zenz.toml")) {
        FCITX_UNUSED(instance);
    }

    ~SkkZenzEngine() override { skk_zenz_backend_free(backend_); }

    void keyEvent(
        const fcitx::InputMethodEntry &entry,
        fcitx::KeyEvent &event) override {
        FCITX_UNUSED(entry);
        if (!backend_ || event.isRelease()) {
            return;
        }

        const auto candidateIndex = event.key().digitSelection();
        if (candidateIndex >= 0 && candidateIndex < 9 &&
            candidatePageOffset_ +
                    static_cast<std::size_t>(candidateIndex) <
                candidateCount_) {
            selectCandidate(
                event.inputContext(),
                candidatePageOffset_ +
                    static_cast<std::size_t>(candidateIndex));
            event.filterAndAccept();
            return;
        }

        synchronizeContext(event.inputContext());
        const auto &rawKey = event.rawKey();
        const auto text = inputText(rawKey);
        const auto keyName = logicalKeyName(event.key());
        UpdateTarget target{this, event.inputContext(), &event};
        skk_zenz_backend_key(
            backend_, text.empty() ? nullptr : text.c_str(), keyName.c_str(),
            backendModifiers(rawKey.states()), &target, updateCallback);
        if (!event.accepted() && text.empty() && !rawKey.isModifier()) {
            skk_zenz_backend_set_context(
                backend_, "", nullptr, discardUpdate);
        }
    }

    void deactivate(
        const fcitx::InputMethodEntry &entry,
        fcitx::InputContextEvent &event) override {
        FCITX_UNUSED(entry);
        focusOut(event.inputContext());
    }

    void reset(
        const fcitx::InputMethodEntry &entry,
        fcitx::InputContextEvent &event) override {
        FCITX_UNUSED(entry);
        if (!backend_) {
            return;
        }
        UpdateTarget target{this, event.inputContext(), nullptr};
        skk_zenz_backend_reset(backend_, &target, updateCallback);
    }

    void applyUpdate(
        fcitx::InputContext *inputContext, fcitx::KeyEvent *keyEvent,
        const SkkZenzUpdate &update) {
        if (update.commit) {
            inputContext->commitString(update.commit);
        }

        auto &panel = inputContext->inputPanel();
        const fcitx::Text preedit(update.preedit);
        panel.setClientPreedit(preedit);
        panel.setPreedit(preedit);

        if (update.candidateCount == 0) {
            candidateCount_ = 0;
            candidatePageOffset_ = 0;
            panel.setCandidateList(nullptr);
        } else {
            candidateCount_ = update.candidateCount;
            candidatePageOffset_ =
                update.selectedCandidate < update.candidateCount
                    ? update.selectedCandidate / 9 * 9
                    : 0;
            auto candidates = std::make_unique<fcitx::CommonCandidateList>();
            candidates->setPageSize(9);
            candidates->setLayoutHint(
                fcitx::CandidateLayoutHint::Vertical);
            candidates->setSelectionKey({
                fcitx::Key(FcitxKey_1), fcitx::Key(FcitxKey_2),
                fcitx::Key(FcitxKey_3), fcitx::Key(FcitxKey_4),
                fcitx::Key(FcitxKey_5), fcitx::Key(FcitxKey_6),
                fcitx::Key(FcitxKey_7), fcitx::Key(FcitxKey_8),
                fcitx::Key(FcitxKey_9),
            });
            for (std::size_t index = 0; index < update.candidateCount; ++index) {
                const auto &candidate = update.candidates[index];
                candidates->append<CallbackCandidate>(
                    fcitx::Text(candidate.text),
                    fcitx::Text(
                        candidate.annotation ? candidate.annotation : ""),
                    [this, index](fcitx::InputContext *context) {
                        selectCandidate(context, index);
                    });
            }
            if (update.selectedCandidate < update.candidateCount) {
                candidates->setGlobalCursorIndex(
                    static_cast<int>(update.selectedCandidate));
            }
            panel.setCandidateList(std::move(candidates));
        }

        inputContext->updatePreedit();
        inputContext->updateUserInterface(
            fcitx::UserInterfaceComponent::InputPanel);
        if (keyEvent && update.consumed) {
            keyEvent->filterAndAccept();
        }
    }

private:
    static void updateCallback(
        void *userData, const SkkZenzUpdate *update) {
        auto *target = static_cast<UpdateTarget *>(userData);
        target->engine->applyUpdate(
            target->inputContext, target->keyEvent, *update);
    }

    static void discardUpdate(
        void *userData, const SkkZenzUpdate *update) {
        FCITX_UNUSED(userData);
        FCITX_UNUSED(update);
    }

    void synchronizeContext(fcitx::InputContext *inputContext) {
        const auto &surrounding = inputContext->surroundingText();
        if (!surrounding.isValid()) {
            return;
        }
        const auto &text = surrounding.text();
        const auto cursor = static_cast<std::size_t>(surrounding.cursor());
        auto iterator = text.begin();
        for (std::size_t index = 0;
             index < cursor && iterator != text.end(); ++index) {
            const auto byte = static_cast<unsigned char>(*iterator);
            iterator += byte < 0x80   ? 1
                        : byte < 0xe0 ? 2
                        : byte < 0xf0 ? 3
                                      : 4;
        }
        const std::string left(text.begin(), iterator);
        skk_zenz_backend_set_context(
            backend_, left.c_str(), nullptr, discardUpdate);
    }

    void focusOut(fcitx::InputContext *inputContext) {
        if (!backend_) {
            return;
        }
        UpdateTarget target{this, inputContext, nullptr};
        skk_zenz_backend_focus_out(backend_, &target, updateCallback);
    }

    void selectCandidate(
        fcitx::InputContext *inputContext, std::size_t index) {
        UpdateTarget target{this, inputContext, nullptr};
        skk_zenz_backend_select_candidate(
            backend_, index, &target, updateCallback);
    }

    SkkZenzBackend *backend_;
    std::size_t candidateCount_ = 0;
    std::size_t candidatePageOffset_ = 0;
};

class SkkZenzEngineFactory final : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new SkkZenzEngine(manager->instance());
    }
};

} // namespace

FCITX_ADDON_FACTORY(SkkZenzEngineFactory);
