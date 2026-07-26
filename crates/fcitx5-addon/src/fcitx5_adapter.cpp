#include <atomic>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <functional>
#include <iomanip>
#include <iterator>
#include <memory>
#include <mutex>
#include <optional>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>
#include <fcitx/action.h>
#include <fcitx/candidatelist.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/menu.h>
#include <fcitx/statusarea.h>
#include <fcitx/surroundingtext.h>
#include <fcitx/text.h>
#include <fcitx/userinterfacemanager.h>
#include <notifications_public.h>
#include <fcitx-utils/event.h>
#include <fcitx-utils/key.h>

#include "backend_ffi.h"
#include "context_tracker.h"

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

struct InputModeStatus {
    const char *label;
    const char *description;
    const char *actionName;
};

constexpr InputModeStatus INPUT_MODE_STATUS[] = {
    {"あ", "Hiragana", "hiragana"},
    {"ア", "Katakana", "katakana"},
    {"ｱ", "Half width Katakana", "hankaku-katakana"},
    {"A_", "Latin", "latin"},
    {"Ａ", "Wide latin", "wide-latin"},
};

const InputModeStatus &inputModeStatus(std::uint8_t mode) {
    return INPUT_MODE_STATUS[mode < std::size(INPUT_MODE_STATUS) ? mode : 0];
}

class ModeAction final : public fcitx::Action {
public:
    explicit ModeAction(std::function<std::uint8_t()> currentMode)
        : currentMode_(std::move(currentMode)) {}

    std::string shortText(fcitx::InputContext *) const override {
        const auto &status = inputModeStatus(currentMode_());
        return std::string(status.label) + " - " + status.description;
    }

    std::string longText(fcitx::InputContext *) const override {
        return inputModeStatus(currentMode_()).description;
    }

    std::string icon(fcitx::InputContext *) const override {
        return "";
    }

private:
    std::function<std::uint8_t()> currentMode_;
};

class ModeSubAction final : public fcitx::SimpleAction {
public:
    ModeSubAction(
        std::uint8_t mode, std::function<std::uint8_t()> currentMode,
        std::function<void(fcitx::InputContext *, std::uint8_t)> setMode)
        : mode_(mode), currentMode_(std::move(currentMode)),
          setMode_(std::move(setMode)) {
        const auto &status = inputModeStatus(mode);
        setShortText(std::string(status.label) + " - " + status.description);
        setLongText(status.description);
        setCheckable(true);
    }

    bool isChecked(fcitx::InputContext *) const override {
        return mode_ == currentMode_();
    }

    void activate(fcitx::InputContext *inputContext) override {
        setMode_(inputContext, mode_);
    }

private:
    std::uint8_t mode_;
    std::function<std::uint8_t()> currentMode_;
    std::function<void(fcitx::InputContext *, std::uint8_t)> setMode_;
};

class SkkZenzEngine final : public fcitx::InputMethodEngineV2 {
public:
    explicit SkkZenzEngine(fcitx::Instance *instance)
        : instance_(instance),
          initializationThread_([this]() {
              backend_.store(
                  skk_zenz_backend_new(
                      nullptr, this, modelDownloadCallback),
                  std::memory_order_release);
              initializationFinished_.store(true, std::memory_order_release);
          }) {
        modeAction_ = std::make_unique<ModeAction>(
            [this]() { return lastInputMode_; });
        modeMenu_ = std::make_unique<fcitx::Menu>();
        modeAction_->setMenu(modeMenu_.get());
        instance_->userInterfaceManager().registerAction(
            "skk-zenz-input-mode", modeAction_.get());
        for (std::uint8_t mode = 0; mode < std::size(INPUT_MODE_STATUS);
             ++mode) {
            modeSubActions_.push_back(std::make_unique<ModeSubAction>(
                mode, [this]() { return lastInputMode_; },
                [this](
                    fcitx::InputContext *inputContext,
                    std::uint8_t selectedMode) {
                    setInputMode(inputContext, selectedMode);
                }));
            instance_->userInterfaceManager().registerAction(
                std::string("skk-zenz-input-mode-") +
                    inputModeStatus(mode).actionName,
                modeSubActions_.back().get());
            modeMenu_->addAction(modeSubActions_.back().get());
        }
        rerankTimer_ = instance_->eventLoop().addTimeEvent(
            CLOCK_MONOTONIC,
            fcitx::now(CLOCK_MONOTONIC) + 16'000, 0,
            [this](
                fcitx::EventSourceTime *timer, std::uint64_t) {
                auto *backend =
                    backend_.load(std::memory_order_acquire);
                if (!backend || !rerankContext_) {
                    timer->setEnabled(false);
                    return true;
                }
                UpdateTarget target{this, rerankContext_, nullptr};
                if (skk_zenz_backend_poll_rerank(
                        backend, &target, updateCallback)) {
                    timer->setNextInterval(16'000);
                    timer->setOneShot();
                } else {
                    rerankContext_ = nullptr;
                    timer->setEnabled(false);
                }
                return true;
            });
        rerankTimer_->setEnabled(false);
        modelReloadTimer_ = instance_->eventLoop().addTimeEvent(
            CLOCK_MONOTONIC,
            fcitx::now(CLOCK_MONOTONIC) + 100'000, 0,
            [this](
                fcitx::EventSourceTime *timer, std::uint64_t) {
                auto *backend =
                    backend_.load(std::memory_order_acquire);
                showPendingModelDownloadMessages();
                if (!backend ||
                    !skk_zenz_backend_poll_model_reload(
                        backend, this, configMessageCallback)) {
                    timer->setEnabled(false);
                } else {
                    timer->setNextInterval(100'000);
                    timer->setOneShot();
                }
                return true;
            });
        modelReloadTimer_->setEnabled(false);
        initializationTimer_ = instance_->eventLoop().addTimeEvent(
            CLOCK_MONOTONIC,
            fcitx::now(CLOCK_MONOTONIC) + 100'000, 0,
            [this](
                fcitx::EventSourceTime *timer, std::uint64_t) {
                showPendingModelDownloadMessages();
                if (initializationFinished_.load(std::memory_order_acquire)) {
                    showPendingModelDownloadMessages();
                    timer->setEnabled(false);
                } else {
                    timer->setNextInterval(100'000);
                    timer->setOneShot();
                }
                return true;
            });
        eventHandlers_.emplace_back(instance_->watchEvent(
            fcitx::EventType::InputContextKeyEvent,
            fcitx::EventWatcherPhase::PostInputMethod,
            [this](fcitx::Event &event) {
                trackUnconsumedKey(
                    static_cast<fcitx::KeyEvent &>(event));
            }));
        eventHandlers_.emplace_back(instance_->watchEvent(
            fcitx::EventType::InputContextCommitString,
            fcitx::EventWatcherPhase::Default,
            [this](fcitx::Event &event) {
                auto &commit =
                    static_cast<fcitx::CommitStringEvent &>(event);
                trackCommit(
                    commit.inputContext(), commit.text(), std::nullopt);
            }));
        eventHandlers_.emplace_back(instance_->watchEvent(
            fcitx::EventType::InputContextCommitStringWithCursor,
            fcitx::EventWatcherPhase::Default,
            [this](fcitx::Event &event) {
                auto &commit =
                    static_cast<fcitx::CommitStringWithCursorEvent &>(
                        event);
                trackCommit(
                    commit.inputContext(), commit.text(),
                    static_cast<std::size_t>(commit.cursor()));
            }));
        eventHandlers_.emplace_back(instance_->watchEvent(
            fcitx::EventType::InputContextSurroundingTextUpdated,
            fcitx::EventWatcherPhase::Default,
            [this](fcitx::Event &event) {
                auto &contextEvent =
                    static_cast<fcitx::InputContextEvent &>(event);
                trackSurroundingText(contextEvent.inputContext());
            }));
    }

    void activate(
        const fcitx::InputMethodEntry &entry,
        fcitx::InputContextEvent &event) override {
        FCITX_UNUSED(entry);
        event.inputContext()->statusArea().addAction(
            fcitx::StatusGroup::InputMethod, modeAction_.get());
        ensureConfigWatcher();
    }

    ~SkkZenzEngine() override {
        configEvent_.reset();
        initializationTimer_.reset();
        modelReloadTimer_.reset();
        rerankTimer_.reset();
        initializationThread_.join();
        skk_zenz_backend_free(
            backend_.load(std::memory_order_acquire));
    }

    void keyEvent(
        const fcitx::InputMethodEntry &entry,
        fcitx::KeyEvent &event) override {
        FCITX_UNUSED(entry);
        auto *backend = backend_.load(std::memory_order_acquire);
        if (!backend || event.isRelease()) {
            return;
        }
        ensureConfigWatcher();

        const auto candidateIndex = event.key().digitSelection();
        if (candidateWindowVisible_ && candidateIndex >= 0 &&
            candidateIndex < 9 &&
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
            backend, text.empty() ? nullptr : text.c_str(), keyName.c_str(),
            backendModifiers(rawKey.states()), &target, updateCallback);
        if (!event.accepted() && text.empty() && !rawKey.isModifier()) {
            if (rawKey.sym() == FcitxKey_BackSpace) {
                auto context = contextTracker_.context();
                ContextTracker::popLastUtf8(context);
                skk_zenz_backend_set_context(
                    backend, context.c_str(), nullptr, discardUpdate);
                return;
            }
            skk_zenz_backend_set_context(
                backend, "", nullptr, discardUpdate);
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
        auto *backend = backend_.load(std::memory_order_acquire);
        if (!backend) {
            return;
        }
        UpdateTarget target{this, event.inputContext(), nullptr};
        skk_zenz_backend_reset(backend, &target, updateCallback);
    }

    void applyUpdate(
        fcitx::InputContext *inputContext, fcitx::KeyEvent *keyEvent,
        const SkkZenzUpdate &update) {
        updateRerankPolling(inputContext, update.rerankPending);
        if (update.commit) {
            inputContext->commitString(update.commit);
        }

        const bool modeChanged = update.inputMode != lastInputMode_;
        if (modeChanged) {
            lastInputMode_ = update.inputMode;
            modeAction_->update(inputContext);
        }

        auto &panel = inputContext->inputPanel();
        candidateWindowVisible_ = update.candidateWindowVisible;
        std::string auxiliary;
        if (!update.candidateWindowVisible &&
            update.selectedCandidate < update.candidateCount) {
            const auto &candidate =
                update.candidates[update.selectedCandidate];
            auxiliary = candidate.text;
            if (update.showAnnotations && candidate.annotation &&
                candidate.annotation[0] != '\0') {
                auxiliary += " [";
                auxiliary += candidate.annotation;
                auxiliary += "]";
            }
        }
        if (update.rerankPending) {
            if (!auxiliary.empty()) {
                auxiliary += "  ";
            }
            auxiliary +=
                "AI " +
                std::to_string(update.evaluatedCandidateCount) + "/" +
                std::to_string(update.totalCandidateCount);
        }
        panel.setAuxDown(fcitx::Text(auxiliary));
        if (modeChanged && update.preedit[0] == '\0' &&
            update.candidateCount == 0) {
            candidateCount_ = 0;
            candidatePageOffset_ = 0;
            panel.reset();
            inputContext->updatePreedit();
            instance_->showInputMethodInformation(inputContext);
            inputContext->updateUserInterface(
                fcitx::UserInterfaceComponent::InputPanel);
            if (keyEvent && update.consumed) {
                keyEvent->filterAndAccept();
            }
            return;
        }

        const fcitx::Text preedit(update.preedit);
        if (inputContext->capabilityFlags().test(
                fcitx::CapabilityFlag::Preedit)) {
            panel.setClientPreedit(preedit);
            panel.setPreedit(fcitx::Text());
        } else {
            panel.setClientPreedit(fcitx::Text());
            panel.setPreedit(preedit);
        }

        if (update.candidateCount == 0) {
            candidateCount_ = 0;
            candidatePageOffset_ = 0;
            panel.setCandidateList(nullptr);
        } else if (!update.candidateWindowVisible) {
            candidateCount_ = update.candidateCount;
            candidatePageOffset_ = 0;
            panel.setCandidateList(nullptr);
        } else {
            candidateCount_ = update.candidateCount;
            const auto relativeSelected =
                update.selectedCandidate - update.candidatePageStart;
            candidatePageOffset_ =
                update.selectedCandidate < update.candidateCount
                    ? update.candidatePageStart +
                          relativeSelected / update.candidatePageSize *
                              update.candidatePageSize
                    : update.candidatePageStart;
            auto candidates = std::make_unique<fcitx::CommonCandidateList>();
            candidates->setPageSize(
                static_cast<int>(update.candidatePageSize));
            candidates->setLayoutHint(
                fcitx::CandidateLayoutHint::Vertical);
            candidates->setSelectionKey({
                fcitx::Key(FcitxKey_1), fcitx::Key(FcitxKey_2),
                fcitx::Key(FcitxKey_3), fcitx::Key(FcitxKey_4),
                fcitx::Key(FcitxKey_5), fcitx::Key(FcitxKey_6),
                fcitx::Key(FcitxKey_7), fcitx::Key(FcitxKey_8),
                fcitx::Key(FcitxKey_9),
            });
            for (
                std::size_t index = update.candidatePageStart;
                index < update.candidateCount; ++index) {
                const auto &candidate = update.candidates[index];
                candidates->append<CallbackCandidate>(
                    fcitx::Text(candidate.text),
                    fcitx::Text(
                        update.showAnnotations && candidate.annotation
                            ? candidate.annotation
                            : ""),
                    [this, index](fcitx::InputContext *context) {
                        selectCandidate(context, index);
                    });
            }
            if (update.selectedCandidate < update.candidateCount) {
                candidates->setGlobalCursorIndex(
                    static_cast<int>(relativeSelected));
                candidates->setPage(
                    static_cast<int>(
                        relativeSelected / update.candidatePageSize));
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
    struct ModelDownloadMessage {
        std::uint8_t kind;
        std::string text;
    };

    static void configMessageCallback(
        void *userData, bool success, const char *message) {
        static_cast<SkkZenzEngine *>(userData)->showConfigMessage(
            success, message);
    }

    static void modelDownloadCallback(
        void *userData, std::uint8_t kind, const char *message) {
        auto *engine = static_cast<SkkZenzEngine *>(userData);
        std::lock_guard lock(engine->modelDownloadMessagesMutex_);
        engine->modelDownloadMessages_.push_back(
            {kind, message ? message : ""});
    }

    void showPendingModelDownloadMessages() {
        std::deque<ModelDownloadMessage> messages;
        {
            std::lock_guard lock(modelDownloadMessagesMutex_);
            messages.swap(modelDownloadMessages_);
        }
        for (const auto &message : messages) {
            showModelDownloadMessage(message);
        }
    }

    void showModelDownloadMessage(const ModelDownloadMessage &message) {
        auto *notifications =
            instance_->addonManager().addon("notifications", true);
        if (!notifications) {
            return;
        }
        notifications->call<fcitx::INotifications::showTip>(
            "skk-zenz-model-download", "SKK Zenz", "",
            message.kind == 2 ? "SKK Zenz model download error"
                              : "SKK Zenz model download",
            message.text, 5000);
    }

    void showConfigMessage(bool success, const char *message) {
        auto *notifications =
            instance_->addonManager().addon("notifications", true);
        if (!notifications) {
            return;
        }
        notifications->call<fcitx::INotifications::showTip>(
            "skk-zenz-config", "SKK Zenz", "",
            success ? "SKK Zenz configuration"
                    : "SKK Zenz configuration error",
            message ? message : "", 5000);
    }

    void ensureConfigWatcher() {
        if (configEvent_) {
            return;
        }
        auto *backend = backend_.load(std::memory_order_acquire);
        if (!backend) {
            return;
        }
        const int fd = skk_zenz_backend_config_watch_fd(backend);
        if (fd < 0) {
            return;
        }
        configEvent_ = instance_->eventLoop().addIOEvent(
            fd, fcitx::IOEventFlag::In,
            [this](
                fcitx::EventSourceIO *, int,
                fcitx::IOEventFlags) {
                auto *backend =
                    backend_.load(std::memory_order_acquire);
                if (!backend) {
                    return true;
                }
                skk_zenz_backend_reload_config_if_changed(
                    backend, this, configMessageCallback);
                if (skk_zenz_backend_poll_model_reload(
                        backend, this, configMessageCallback)) {
                    modelReloadTimer_->setNextInterval(100'000);
                    modelReloadTimer_->setOneShot();
                    modelReloadTimer_->setEnabled(true);
                }
                return true;
            });
    }

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
        auto *backend = backend_.load(std::memory_order_acquire);
        if (!backend) {
            return;
        }
        contextTracker_.trackSurroundingText(inputContext);
        skk_zenz_backend_set_context(
            backend, contextTracker_.context().c_str(), nullptr, discardUpdate);
    }

    void trackSurroundingText(fcitx::InputContext *inputContext) {
        contextTracker_.trackSurroundingText(inputContext);
    }

    void trackCommit(
        fcitx::InputContext *inputContext, const std::string &text,
        std::optional<std::size_t> cursor) {
        contextTracker_.trackCommit(inputContext, text, cursor);
    }

    void trackUnconsumedKey(fcitx::KeyEvent &event) {
        contextTracker_.trackUnconsumedKey(event);
    }

    void focusOut(fcitx::InputContext *inputContext) {
        auto *backend = backend_.load(std::memory_order_acquire);
        if (!backend) {
            return;
        }
        UpdateTarget target{this, inputContext, nullptr};
        skk_zenz_backend_focus_out(backend, &target, updateCallback);
    }

    void selectCandidate(
        fcitx::InputContext *inputContext, std::size_t index) {
        auto *backend = backend_.load(std::memory_order_acquire);
        if (!backend) {
            return;
        }
        UpdateTarget target{this, inputContext, nullptr};
        skk_zenz_backend_select_candidate(
            backend, index, &target, updateCallback);
    }

    void setInputMode(
        fcitx::InputContext *inputContext, std::uint8_t mode) {
        auto *backend = backend_.load(std::memory_order_acquire);
        if (!backend) {
            return;
        }
        UpdateTarget target{this, inputContext, nullptr};
        skk_zenz_backend_set_input_mode(
            backend, mode, &target, updateCallback);
    }

    void updateRerankPolling(
        fcitx::InputContext *inputContext, bool pending) {
        if (!pending) {
            if (rerankContext_ == inputContext) {
                rerankContext_ = nullptr;
                rerankTimer_->setEnabled(false);
            }
            return;
        }
        rerankContext_ = inputContext;
        rerankTimer_->setNextInterval(16'000);
        rerankTimer_->setOneShot();
        rerankTimer_->setEnabled(true);
    }

    std::string subMode(
        const fcitx::InputMethodEntry &,
        fcitx::InputContext &) override {
        return inputModeStatus(lastInputMode_).description;
    }

    std::string subModeLabelImpl(
        const fcitx::InputMethodEntry &,
        fcitx::InputContext &) override {
        return inputModeStatus(lastInputMode_).label;
    }

    fcitx::Instance *instance_;
    std::mutex modelDownloadMessagesMutex_;
    std::deque<ModelDownloadMessage> modelDownloadMessages_;
    std::atomic<SkkZenzBackend *> backend_{nullptr};
    std::atomic<bool> initializationFinished_{false};
    std::thread initializationThread_;
    std::vector<std::unique_ptr<
        fcitx::HandlerTableEntry<fcitx::EventHandler>>>
        eventHandlers_;
    ContextTracker contextTracker_;
    std::size_t candidateCount_ = 0;
    std::size_t candidatePageOffset_ = 0;
    bool candidateWindowVisible_ = false;
    std::uint8_t lastInputMode_ = 0;
    std::unique_ptr<ModeAction> modeAction_;
    std::unique_ptr<fcitx::Menu> modeMenu_;
    std::vector<std::unique_ptr<ModeSubAction>> modeSubActions_;
    fcitx::InputContext *rerankContext_ = nullptr;
    std::unique_ptr<fcitx::EventSourceTime> rerankTimer_;
    std::unique_ptr<fcitx::EventSourceIO> configEvent_;
    std::unique_ptr<fcitx::EventSourceTime> initializationTimer_;
    std::unique_ptr<fcitx::EventSourceTime> modelReloadTimer_;
};

class SkkZenzEngineFactory final : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new SkkZenzEngine(manager->instance());
    }
};

} // namespace

FCITX_ADDON_FACTORY(SkkZenzEngineFactory);
