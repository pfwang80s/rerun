const state = globalThis.__rerun_web_viewer_test_state;

class FakeWebHandle {
  constructor(options) {
    this.options = options;
    this.panicked = false;
    state.handles.push(this);
    state.calls.push(["construct", options]);
  }

  async start(canvas) {
    state.calls.push(["start", canvas]);
    if (state.startError) {
      throw state.startError;
    }
  }

  async start_with_requests(canvas, specs) {
    state.calls.push(["start_with_requests", canvas, specs]);
    if (state.startWithRequestsError) {
      throw state.startWithRequestsError;
    }
    return state.startWithRequestsResult;
  }

  remote_page_hidden_v1(epoch) { state.calls.push(["remote_page_hidden_v1", epoch]); }
  remote_page_resume_v1(from, to, nonce) { state.calls.push(["remote_page_resume_v1", from, to, nonce]); }
  remote_page_deadline_v1(epoch, deadline) { state.calls.push(["remote_page_deadline_v1", epoch, deadline]); }
  remote_page_terminate_v1(epoch) { state.calls.push(["remote_page_terminate_v1", epoch]); }

  add_receiver(url) {
    state.calls.push(["add_receiver", url]);
    if (state.addReceiverErrorAt === state.addReceiverCalls++) {
      throw new Error("injected add_receiver failure");
    }
  }

  remove_receiver(url) {
    state.calls.push(["remove_receiver", url]);
  }

  open_channel(id, name) {
    state.calls.push(["open_channel", id, name]);
  }

  send_rrd_to_channel(id, data) {
    state.calls.push(["send_rrd_to_channel", id, data]);
  }

  send_table_to_channel(id, data) {
    state.calls.push(["send_table_to_channel", id, data]);
  }

  close_channel(id) {
    state.calls.push(["close_channel", id]);
  }

  get_active_recording_id() {
    state.calls.push(["get_active_recording_id"]);
    return state.activeRecordingId;
  }

  set_active_recording_id(recordingId) {
    state.calls.push(["set_active_recording_id", recordingId]);
  }

  get_playing(recordingId) {
    state.calls.push(["get_playing", recordingId]);
    return state.playing;
  }

  set_playing(recordingId, playing) {
    state.calls.push(["set_playing", recordingId, playing]);
  }

  get_time_for_timeline(recordingId, timeline) {
    state.calls.push(["get_time_for_timeline", recordingId, timeline]);
    return state.currentTime;
  }

  set_time_for_timeline(recordingId, timeline, time) {
    state.calls.push(["set_time_for_timeline", recordingId, timeline, time]);
  }

  get_active_timeline(recordingId) {
    state.calls.push(["get_active_timeline", recordingId]);
    return state.activeTimeline;
  }

  set_active_timeline(recordingId, timeline) {
    state.calls.push(["set_active_timeline", recordingId, timeline]);
  }

  get_timeline_time_range(recordingId, timeline) {
    state.calls.push(["get_timeline_time_range", recordingId, timeline]);
    return state.timelineTimeRange;
  }

  has_panicked() {
    return this.panicked;
  }

  panic_message() {
    return null;
  }

  destroy() {
    state.calls.push(["destroy"]);
  }

  free() {
    state.calls.push(["free"]);
  }

  emit(event) {
    this.options.on_viewer_event(JSON.stringify(event));
  }
}

async function fakeBindgen() {}

fakeBindgen.WebHandle = FakeWebHandle;
fakeBindgen.deinit = () => state.calls.push(["deinit"]);

export default function loadFakeBindgen() {
  return fakeBindgen;
}
