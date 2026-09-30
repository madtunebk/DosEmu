# Suggestion - Late Features and Improvements

## Phase 1: polish after MVP

### 1. Better video pipeline
- replace polling with event-based or dirty-rectangle updates
- compress frames before sending
- support frame scaling and resize in client
- add latency control and bitrate/fps tuning

### 2. Mouse support
- capture pointer movement
- support left/right click and wheel
- map local pointer events to QEMU input events

### 3. Keyboard quality improvements
- full key mapping for DOS apps and games
- modifier combos and multi-key states
- handling of autorepeat and focus state
- support for scan codes / qcodes more explicitly

### 4. Local UI improvement
- proper window resize
- status overlays
- FPS and latency indicator
- reconnect / error recovery logic

## Phase 2: browser product

### 5. Browser client
- video stream over WebSocket / HTTP streaming
- display in browser canvas or canvas-like element
- keyboard and mouse events forwarded to backend
- session management and connection state

### 6. Web UX
- fullscreen mode
- responsive layout
- on-screen virtual keyboard for special keys
- copy/paste support
- command palette / quick actions

## Phase 3: richer VM integration

### 7. Audio streaming
- capture audio from QEMU or host bridge
- PCM buffering and resampling
- stream to browser audio element
- support mute, volume, and latency tuning

### 8. Session resilience
- automatic reconnect
- graceful shutdown and cleanup
- persistent machine metadata
- health checks and diagnostics

## Phase 4: advanced features

### 9. Multi-machine support
- multiple DOS VM instances
- selector for running sessions
- per-session state and config

### 10. Disk / snapshot controls
- save state
- restore snapshot
- attach/detach disk images
- hot switch virtual hardware config

### 11. Resource monitoring
- CPU, RAM, disk activity
- FPS counter
- network/IO diagnostics
- debugging mode for QMP traffic

## Strategic guideline

The priority should stay:
1. boot DOS reliably
2. capture screen
3. send keyboard input
4. improve latency and responsiveness
5. add browser streaming
6. add audio and advanced features

This keeps the project grounded in a working MVP instead of building too much early.

## Long-term vision

The end state is a Rust bridge that:
- runs QEMU headless
- exposes display and input over a clean protocol
- streams video/audio to a browser client
- forwards user input in real time
- supports a clean, modern remote-DOS experience

This is the real direction to aim for, but only after the minimal working loop is proven.
