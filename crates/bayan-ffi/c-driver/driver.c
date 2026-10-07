/*
 * The C test driver of the bayan-core C SDK (work package CORE-007): a plain C11 program that uses the engine only
 * through bayan_ffi.h, the way a shell does, and goes through handshake → open → render a tile → record → replay,
 * then the panic path. `cargo xtask c-driver` builds it twice, linked against the static and against the dynamic
 * library, and runs both.
 *
 * It prints one line per check and, last, "tile <hash>": the digest of the tile, which the Node.js driver of the
 * WebAssembly package prints for the same tile, so CI can compare native and WebAssembly output across platforms.
 * It exits with status 1 on the first failed check.
 *
 * The engine calls back on its own thread, so replies are handed over with a mutex and a condition variable: POSIX
 * threads on Linux and macOS, and the Win32 equivalents (a slim reader/writer lock and a condition variable, from
 * kernel32) on Windows.
 *
 * SPDX-FileCopyrightText: 2026 BayanDocs contributors
 * SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission
 */

#if !defined(_WIN32) && !defined(__APPLE__)
#define _POSIX_C_SOURCE 200809L
#endif

#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "bayan_ffi.h"

/* ---- Checks ----------------------------------------------------------------------------------------------------- */

static void check(int ok, const char *what) {
  if (!ok) {
    printf("not ok - %s\n", what);
    fflush(stdout);
    exit(1);
  }
}

static void pass(const char *what) { printf("ok - %s\n", what); }

static void *allocate(size_t size) {
  void *memory = malloc(size == 0 ? 1 : size);
  check(memory != NULL, "memory allocation");
  return memory;
}

/* ---- A mutex, a condition variable and a deadline on every platform ------------------------------------------- */

#define WAIT_SECONDS 30

#if defined(_WIN32)
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0A00 /* Windows 10: slim reader/writer locks, condition variables and GetTickCount64 */
#endif
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
typedef SRWLOCK driver_mutex;
typedef CONDITION_VARIABLE driver_condition;
typedef ULONGLONG driver_deadline; /* milliseconds of GetTickCount64, a monotonic clock */
static int mutex_init(driver_mutex *mutex) {
  InitializeSRWLock(mutex);
  return 1;
}
static void mutex_lock(driver_mutex *mutex) { AcquireSRWLockExclusive(mutex); }
static void mutex_unlock(driver_mutex *mutex) { ReleaseSRWLockExclusive(mutex); }
static int condition_init(driver_condition *condition) {
  InitializeConditionVariable(condition);
  return 1;
}
static void condition_wake(driver_condition *condition) { WakeAllConditionVariable(condition); }
static driver_deadline deadline_in(unsigned seconds) { return GetTickCount64() + (ULONGLONG)seconds * 1000; }
/* Waits until woken or until the deadline passes; returns 0 once it has passed. */
static int condition_wait(driver_condition *condition, driver_mutex *mutex, const driver_deadline *deadline) {
  ULONGLONG now = GetTickCount64();
  if (now >= *deadline) {
    return 0;
  }
  return SleepConditionVariableSRW(condition, mutex, (DWORD)(*deadline - now), 0) || GetLastError() != ERROR_TIMEOUT;
}
#else
#include <errno.h>
#include <pthread.h>
typedef pthread_mutex_t driver_mutex;
typedef pthread_cond_t driver_condition;
typedef struct timespec driver_deadline; /* CLOCK_REALTIME, the clock pthread_cond_timedwait uses */
static int mutex_init(driver_mutex *mutex) { return pthread_mutex_init(mutex, NULL) == 0; }
static void mutex_lock(driver_mutex *mutex) { (void)pthread_mutex_lock(mutex); }
static void mutex_unlock(driver_mutex *mutex) { (void)pthread_mutex_unlock(mutex); }
static int condition_init(driver_condition *condition) { return pthread_cond_init(condition, NULL) == 0; }
static void condition_wake(driver_condition *condition) { (void)pthread_cond_broadcast(condition); }
static driver_deadline deadline_in(unsigned seconds) {
  struct timespec deadline;
  check(clock_gettime(CLOCK_REALTIME, &deadline) == 0, "reading the clock");
  deadline.tv_sec += (time_t)seconds;
  return deadline;
}
/* Waits until woken or until the deadline passes; returns 0 once it has passed. */
static int condition_wait(driver_condition *condition, driver_mutex *mutex, const driver_deadline *deadline) {
  return pthread_cond_timedwait(condition, mutex, deadline) != ETIMEDOUT;
}
#endif

/* ---- The engine's messages, collected from its thread ----------------------------------------------------------- */

typedef struct {
  driver_mutex mutex;
  driver_condition arrived;
  char **messages;
  size_t count;
  size_t capacity;
} Collector;

/* The message callback: runs on the engine thread, copies the message and wakes the main thread (spec §3.1, §8). */
static void on_message(void *user_data, const uint8_t *json, size_t json_len) {
  Collector *collector = (Collector *)user_data;
  char *copy = (char *)allocate(json_len + 1);
  memcpy(copy, json, json_len);
  copy[json_len] = '\0';
  mutex_lock(&collector->mutex);
  if (collector->count == collector->capacity) {
    size_t capacity = collector->capacity == 0 ? 16 : collector->capacity * 2;
    char **grown = (char **)realloc(collector->messages, capacity * sizeof *grown);
    check(grown != NULL, "memory allocation");
    collector->messages = grown;
    collector->capacity = capacity;
  }
  collector->messages[collector->count] = copy;
  collector->count += 1;
  condition_wake(&collector->arrived);
  mutex_unlock(&collector->mutex);
}

/* Waits for the first message that contains `needle`, takes it out of the collector and returns it; free it after. */
static char *wait_for(Collector *collector, const char *needle) {
  driver_deadline deadline = deadline_in(WAIT_SECONDS);
  mutex_lock(&collector->mutex);
  for (;;) {
    for (size_t index = 0; index < collector->count; index += 1) {
      char *message = collector->messages[index];
      if (strstr(message, needle) != NULL) {
        memmove(&collector->messages[index], &collector->messages[index + 1],
                (collector->count - index - 1) * sizeof *collector->messages);
        collector->count -= 1;
        mutex_unlock(&collector->mutex);
        return message;
      }
    }
    if (!condition_wait(&collector->arrived, &collector->mutex, &deadline)) {
      mutex_unlock(&collector->mutex);
      printf("not ok - no message with %s within %d s\n", needle, WAIT_SECONDS);
      exit(1);
    }
  }
}

/* The reply to request `id`: replies start {"v":0,"re":<id>, (spec §4). */
static char *wait_reply(Collector *collector, unsigned id) {
  char needle[32];
  snprintf(needle, sizeof needle, "\"re\":%u,", id);
  return wait_for(collector, needle);
}

/* ---- Minimal JSON reading: the engine's JSON is compact, so a field is "name":value with no spaces ------------- */

static int contains(const char *text, const char *needle) { return strstr(text, needle) != NULL; }

/* The unsigned integer after the first "name":, or 0 if there is none. */
static uint64_t number_field(const char *json, const char *name) {
  char key[64];
  snprintf(key, sizeof key, "\"%s\":", name);
  const char *found = strstr(json, key);
  if (found == NULL) {
    return 0;
  }
  return (uint64_t)strtoull(found + strlen(key), NULL, 10);
}

/* How often `needle` occurs in `text`. */
static size_t occurrences(const char *text, const char *needle) {
  size_t count = 0;
  for (const char *found = strstr(text, needle); found != NULL; found = strstr(found + 1, needle)) {
    count += 1;
  }
  return count;
}

/* ---- The engine's digest, FNV-1a 64 (spec §10) ------------------------------------------------------------------ */

#define FNV_OFFSET UINT64_C(0xcbf29ce484222325)
#define FNV_PRIME UINT64_C(0x100000001b3)

static uint64_t fnv1a64(uint64_t hash, const uint8_t *bytes, size_t length) {
  for (size_t index = 0; index < length; index += 1) {
    hash ^= bytes[index];
    hash *= FNV_PRIME;
  }
  return hash;
}

/* ---- Talking to the engine ------------------------------------------------------------------------------------- */

static void post(BayanEngine *engine, const char *json) {
  check(bayan_engine_post(engine, (const uint8_t *)json, strlen(json)) == BAYAN_STATUS_OK, "bayan_engine_post");
}

/* Copies a blob out of the engine: first asks for its size, then copies it. Returns the bytes; free them after. */
static uint8_t *take_blob(BayanEngine *engine, BayanBlobId blob, size_t *length) {
  check(bayan_blob_get(engine, blob, NULL, 0, length) == BAYAN_STATUS_BUFFER_TOO_SMALL,
        "bayan_blob_get without a buffer reports the size");
  uint8_t *bytes = (uint8_t *)allocate(*length);
  size_t copied = 0;
  check(bayan_blob_get(engine, blob, bytes, *length, &copied) == BAYAN_STATUS_OK && copied == *length,
        "bayan_blob_get copies the blob");
  check(bayan_blob_release(engine, blob) == BAYAN_STATUS_OK, "bayan_blob_release");
  return bytes;
}

/* 1 device-independent pixel is 19,050 BLU at zoom 1 (spec §7). The tile is 64 × 64 pixels, one inch (96 pixels)
 * inside page 0, so it lies entirely on the page and every pixel is opaque: the same tile as the Node.js driver's. */
#define TILE_SIDE 64
#define TILE_REQUEST                                                                                                  \
  "\"doc_id\":%llu,\"page\":0,\"rect\":{\"x\":1828800,\"y\":1828800,\"width\":1219200,\"height\":1219200},"         \
  "\"zoom\":1,\"device_scale\":1"
/* A stride with room to spare, to check that the engine writes only the pixels of each row. */
#define TILE_STRIDE (TILE_SIDE * 4 + 32)

int main(void) {
  char message[512];
  char digest[32];

  const char *version = bayan_version();
  check(version != NULL && version[0] != '\0', "bayan_version");
  printf("engine %s\n", version);

  static const char config[] = "{\"test\":{\"allow_panic\":true}}";
  BayanEngine *engine = bayan_engine_new((const uint8_t *)config, sizeof config - 1);
  check(engine != NULL, "bayan_engine_new");
  Collector collector = {0};
  check(mutex_init(&collector.mutex) && condition_init(&collector.arrived), "a mutex and a condition variable");
  check(bayan_engine_set_callback(engine, on_message, &collector) == BAYAN_STATUS_OK, "bayan_engine_set_callback");

  /* Handshake. */
  post(engine, "{\"v\":0,\"id\":1,\"type\":\"hello\",\"payload\":{\"protocol_versions\":[0]}}");
  char *reply = wait_reply(&collector, 1);
  check(contains(reply, "\"ok\":true") && contains(reply, "\"type\":\"welcome\"") &&
            contains(reply, "\"protocol_version\":0"),
        "hello is answered by welcome with protocol version 0");
  free(reply);
  pass("handshake");

  /* Record everything from here on. */
  post(engine, "{\"v\":0,\"id\":2,\"type\":\"diag.record.start\"}");
  reply = wait_reply(&collector, 2);
  check(contains(reply, "\"ok\":true"), "diag.record.start");
  free(reply);

  /* Open: the file's bytes travel as a blob. Protocol v0 opens the mock document whatever they are. */
  static const char file[] = "any bytes: protocol v0 opens the mock document";
  BayanBlobId blob = bayan_blob_put(engine, (const uint8_t *)file, sizeof file - 1);
  check(blob != 0 && blob % 2 == 1, "bayan_blob_put gives the shell's blob an odd identifier");
  snprintf(message, sizeof message, "{\"v\":0,\"id\":3,\"type\":\"doc.open\",\"payload\":{\"blob\":%llu}}",
           (unsigned long long)blob);
  post(engine, message);
  reply = wait_reply(&collector, 3);
  check(contains(reply, "\"ok\":true") && contains(reply, "\"type\":\"doc.opened\""), "doc.open");
  unsigned long long doc_id = (unsigned long long)number_field(reply, "doc_id");
  free(reply);
  check(doc_id != 0, "doc.opened names the document");
  check(bayan_blob_release(engine, blob) == BAYAN_STATUS_OK, "the shell releases its blob");
  pass("open");

  /* Render a tile into our own buffer, with padded rows. */
  char request[256];
  int request_length = snprintf(request, sizeof request, "{" TILE_REQUEST "}", doc_id);
  check(request_length > 0 && (size_t)request_length < sizeof request, "the tile request fits");
  static uint8_t pixels[TILE_STRIDE * TILE_SIDE];
  memset(pixels, 0xAB, sizeof pixels);
  check(bayan_render_tile(engine, (const uint8_t *)request, (size_t)request_length, pixels, sizeof pixels, TILE_SIDE,
                          TILE_SIDE, TILE_STRIDE) == BAYAN_STATUS_OK,
        "bayan_render_tile");
  uint64_t hash = FNV_OFFSET;
  for (size_t row = 0; row < TILE_SIDE; row += 1) {
    const uint8_t *start = pixels + row * TILE_STRIDE;
    hash = fnv1a64(hash, start, TILE_SIDE * 4);
    for (size_t pixel = 0; pixel < TILE_SIDE; pixel += 1) {
      check(start[pixel * 4 + 3] == 255, "every pixel of a tile inside the page is opaque");
    }
    for (size_t pad = TILE_SIDE * 4; pad < TILE_STRIDE; pad += 1) {
      check(start[pad] == 0xAB, "the bytes between rows stay untouched");
    }
  }
  snprintf(digest, sizeof digest, "fnv1a64:%016" PRIx64, hash);
  check(bayan_render_tile(engine, (const uint8_t *)request, (size_t)request_length, pixels, TILE_SIDE * 4, TILE_SIDE,
                          TILE_SIDE, TILE_STRIDE) == BAYAN_STATUS_BUFFER_TOO_SMALL,
        "bayan_render_tile refuses a buffer that is too small");
  pass("render a tile with bayan_render_tile");

  /* The same tile through a message: the reply carries the digest and a blob with the pixels. */
  snprintf(message, sizeof message,
           "{\"v\":0,\"id\":4,\"type\":\"render.tile\",\"payload\":{" TILE_REQUEST ",\"width\":%d,\"height\":%d}}", doc_id,
           TILE_SIDE, TILE_SIDE);
  post(engine, message);
  reply = wait_reply(&collector, 4);
  char expected[64];
  snprintf(expected, sizeof expected, "\"hash\":\"%s\"", digest);
  check(contains(reply, "\"ok\":true") && contains(reply, expected), "render.tile reports the same digest");
  BayanBlobId tile_blob = (BayanBlobId)number_field(reply, "blob");
  free(reply);
  check(tile_blob != 0 && tile_blob % 2 == 0, "blobs the engine creates have even identifiers");
  size_t tile_length = 0;
  uint8_t *tile = take_blob(engine, tile_blob, &tile_length);
  check(tile_length == TILE_SIDE * TILE_SIDE * 4 && fnv1a64(FNV_OFFSET, tile, tile_length) == hash,
        "the blob holds the same pixels");
  free(tile);
  pass("render the same tile with render.tile");

  /* Stop recording and replay the recording. */
  post(engine, "{\"v\":0,\"id\":5,\"type\":\"diag.record.stop\"}");
  reply = wait_reply(&collector, 5);
  check(contains(reply, "\"ok\":true") && contains(reply, "\"truncated\":false"), "diag.record.stop");
  BayanBlobId recording_blob = (BayanBlobId)number_field(reply, "blob");
  free(reply);
  size_t recording_length = 0;
  uint8_t *recording = take_blob(engine, recording_blob, &recording_length);
  BayanBlobId replay_blob = bayan_blob_put(engine, recording, recording_length);
  free(recording);
  check(replay_blob != 0, "the recording goes back in as a blob");
  snprintf(message, sizeof message, "{\"v\":0,\"id\":6,\"type\":\"diag.replay\",\"payload\":{\"blob\":%llu}}",
           (unsigned long long)replay_blob);
  post(engine, message);
  reply = wait_reply(&collector, 6);
  check(contains(reply, "\"ok\":true") && contains(reply, "\"identical\":true"), "the replay is identical");
  /* Both tiles were recorded: the one from bayan_render_tile and the one from render.tile. */
  check(occurrences(reply, expected) == 2, "the replay renders both tiles with the same digest");
  free(reply);
  check(bayan_blob_release(engine, replay_blob) == BAYAN_STATUS_OK, "release the recording");
  pass("record and replay: identical tile hashes");

  /* A panic in a message handler becomes engine.error, and the engine stays usable. */
  post(engine, "{\"v\":0,\"id\":7,\"type\":\"diag.panic\"}");
  reply = wait_reply(&collector, 7);
  check(contains(reply, "\"ok\":false") && contains(reply, "\"code\":\"panic\""), "diag.panic is answered with panic");
  free(reply);
  reply = wait_for(&collector, "\"type\":\"engine.error\"");
  check(contains(reply, "\"recoverable\":false"), "engine.error says the session is gone");
  free(reply);
  post(engine, "{\"v\":0,\"id\":8,\"type\":\"hello\",\"payload\":{\"protocol_versions\":[0]}}");
  reply = wait_reply(&collector, 8);
  check(contains(reply, "\"ok\":true"), "a new handshake after the panic");
  free(reply);
  pass("panic recovery");

  bayan_engine_free(engine);
  for (size_t index = 0; index < collector.count; index += 1) {
    free(collector.messages[index]);
  }
  free(collector.messages);
  pass("bayan_engine_free");
  printf("tile %s\n", digest);
  return 0;
}
