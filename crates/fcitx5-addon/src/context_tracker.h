#pragma once

#include <cstddef>
#include <iomanip>
#include <optional>
#include <sstream>
#include <string>

#include <fcitx-utils/key.h>
#include <fcitx/inputcontext.h>
#include <fcitx/surroundingtext.h>

class ContextTracker {
public:
    static void popLastUtf8(std::string &text) {
        if (text.empty()) {
            return;
        }
        auto offset = text.size() - 1;
        while (offset > 0 &&
               (static_cast<unsigned char>(text[offset]) & 0xc0) == 0x80) {
            --offset;
        }
        text.erase(offset);
    }

    void switchContext(fcitx::InputContext *inputContext) {
        const auto uuid = uuidString(inputContext);
        if (uuid == activeUuid_) {
            return;
        }
        activeUuid_ = uuid;
        context_.clear();
    }

    void trackSurroundingText(fcitx::InputContext *inputContext) {
        switchContext(inputContext);
        const auto &surrounding = inputContext->surroundingText();
        if (surrounding.isValid()) {
            context_ = leftOfCursor(
                surrounding.text(),
                static_cast<std::size_t>(surrounding.cursor()));
        }
    }

    void trackCommit(
        fcitx::InputContext *inputContext, const std::string &text,
        std::optional<std::size_t> cursor) {
        if (text.empty()) {
            return;
        }
        switchContext(inputContext);
        context_ += cursor ? leftOfCursor(text, *cursor) : text;
    }

    void trackUnconsumedKey(fcitx::KeyEvent &event) {
        if (event.accepted() || event.isRelease()) {
            return;
        }
        switchContext(event.inputContext());
        const auto &key = event.key();
        if (key.isModifier()) {
            return;
        }
        if (key.sym() == FcitxKey_BackSpace) {
            popLastUtf8(context_);
            return;
        }
        if (!key.hasModifier()) {
            const auto text = fcitx::Key::keySymToUTF8(key.sym());
            if (!text.empty() &&
                static_cast<unsigned char>(text.front()) >= 0x20 &&
                text.front() != '\x7f') {
                context_ += text;
                return;
            }
        }
        context_.clear();
    }

    const std::string &context() const {
        return context_;
    }

private:
    static std::string uuidString(
        const fcitx::InputContext *inputContext) {
        std::ostringstream stream;
        stream << std::hex << std::setfill('0');
        for (const auto byte : inputContext->uuid()) {
            stream << std::setw(2) << static_cast<unsigned int>(byte);
        }
        return stream.str();
    }

    static std::string leftOfCursor(
        const std::string &text, std::size_t cursor) {
        auto iterator = text.begin();
        for (std::size_t index = 0;
             index < cursor && iterator != text.end(); ++index) {
            const auto byte = static_cast<unsigned char>(*iterator);
            iterator += byte < 0x80   ? 1
                        : byte < 0xe0 ? 2
                        : byte < 0xf0 ? 3
                                      : 4;
        }
        return {text.begin(), iterator};
    }

    std::string activeUuid_;
    std::string context_;
};
