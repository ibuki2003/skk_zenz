#pragma once

#include <cstddef>
#include <cstdint>

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
    std::uint8_t inputMode;
    bool rerankPending;
    std::size_t evaluatedCandidateCount;
    std::size_t totalCandidateCount;
    bool candidateWindowVisible;
    std::size_t candidatePageStart;
    std::size_t candidatePageSize;
    bool showAnnotations;
};

using SkkZenzUpdateCallback =
    void (*)(void *userData, const SkkZenzUpdate *update);
using SkkZenzMessageCallback =
    void (*)(void *userData, bool success, const char *message);
using SkkZenzModelDownloadCallback =
    void (*)(void *userData, std::uint8_t kind, const char *message);

SkkZenzBackend *skk_zenz_backend_new(
    const char *configPath, void *userData,
    SkkZenzModelDownloadCallback callback);
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
bool skk_zenz_backend_set_input_mode(
    SkkZenzBackend *backend, std::uint8_t mode, void *userData,
    SkkZenzUpdateCallback callback);
bool skk_zenz_backend_poll_rerank(
    SkkZenzBackend *backend, void *userData,
    SkkZenzUpdateCallback callback);
int skk_zenz_backend_config_watch_fd(SkkZenzBackend *backend);
bool skk_zenz_backend_reload_config_if_changed(
    SkkZenzBackend *backend, void *userData,
    SkkZenzMessageCallback callback);
bool skk_zenz_backend_poll_model_reload(
    SkkZenzBackend *backend, void *userData,
    SkkZenzMessageCallback callback);
bool skk_zenz_backend_select_candidate(
    SkkZenzBackend *backend, std::size_t index, void *userData,
    SkkZenzUpdateCallback callback);

}
