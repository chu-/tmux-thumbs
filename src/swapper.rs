extern crate clap;

use self::clap::{App, Arg};
use clap::crate_version;
use regex::Regex;
use std::io::Write;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

trait Executor {
  fn execute(&mut self, args: Vec<String>) -> String;
  fn last_executed(&self) -> Option<Vec<String>>;
}

struct RealShell {
  executed: Option<Vec<String>>,
}

impl RealShell {
  fn new() -> RealShell {
    RealShell { executed: None }
  }
}

impl Executor for RealShell {
  fn execute(&mut self, args: Vec<String>) -> String {
    let execution = Command::new(args[0].as_str())
      .args(&args[1..])
      .output()
      .expect("Couldn't run it");

    self.executed = Some(args);

    let output: String = String::from_utf8_lossy(&execution.stdout).into();

    output.trim_end().to_string()
  }

  fn last_executed(&self) -> Option<Vec<String>> {
    self.executed.clone()
  }
}

const TMP_FILE: &str = "/tmp/thumbs-last";

#[allow(dead_code)]
fn dbg(msg: &str) {
  let mut file = std::fs::OpenOptions::new()
    .create(true)
    .write(true)
    .append(true)
    .open("/tmp/thumbs.log")
    .expect("Unable to open log file");

  writeln!(&mut file, "{}", msg).expect("Unable to write log file");
}

pub struct Swapper<'a> {
  executor: Box<&'a mut dyn Executor>,
  dir: String,
  command: String,
  upcase_command: String,
  multi_command: String,
  url_command: String,
  jump: bool,
  character: Option<String>,
  backward: bool,
  osc52: bool,
  active_pane_id: Option<String>,
  active_pane_in_mode: bool,
  active_pane_cursor: Option<(i32, i32)>,
  active_pane_width: Option<i32>,
  active_pane_height: Option<i32>,
  active_pane_scroll_position: Option<i32>,
  active_pane_history_size: Option<i32>,
  active_pane_zoomed: Option<bool>,
  active_pane_captured_content: Option<String>,
  thumbs_pane_id: Option<String>,
  content: Option<String>,
  ready_signal: String,
  signal: String,
}

impl<'a> Swapper<'a> {
  fn new(
    executor: Box<&'a mut dyn Executor>,
    dir: String,
    command: String,
    upcase_command: String,
    multi_command: String,
    url_command: String,
    jump: bool,
    character: Option<String>,
    backward: bool,
    osc52: bool,
  ) -> Swapper {
    let since_the_epoch = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .expect("Time went backwards");
    let signal = format!("thumbs-finished-{}-{}", since_the_epoch.as_secs(), std::process::id());
    let ready_signal = format!("thumbs-ready-{}-{}", since_the_epoch.as_secs(), std::process::id());

    Swapper {
      executor,
      dir,
      command,
      upcase_command,
      multi_command,
      url_command,
      jump,
      character,
      backward,
      osc52,
      active_pane_id: None,
      active_pane_in_mode: false,
      active_pane_cursor: None,
      active_pane_width: None,
      active_pane_height: None,
      active_pane_scroll_position: None,
      active_pane_history_size: None,
      active_pane_zoomed: None,
      active_pane_captured_content: None,
      thumbs_pane_id: None,
      content: None,
      ready_signal,
      signal,
    }
  }

  pub fn capture_active_pane(&mut self) {
    let active_command = vec![
      "tmux",
      "list-panes",
      "-F",
      "#{pane_id}:#{?pane_in_mode,1,0}:#{pane_height}:#{scroll_position}:#{window_zoomed_flag}:#{?pane_active,active,nope}:#{pane_width}",
    ];

    let output = self
      .executor
      .execute(active_command.iter().map(|arg| arg.to_string()).collect());

    let lines: Vec<&str> = output.split('\n').collect();
    let chunks: Vec<Vec<&str>> = lines.into_iter().map(|line| line.split(':').collect()).collect();

    let active_pane = chunks
      .iter()
      .find(|&chunks| *chunks.get(5).unwrap() == "active")
      .expect("Unable to find active pane");

    let pane_id = active_pane.get(0).unwrap();
    let active_pane_was_in_mode = active_pane.get(1).unwrap() == &"1";

    self.active_pane_id = Some(pane_id.to_string());
    self.active_pane_in_mode = active_pane_was_in_mode;

    let pane_height = active_pane
      .get(2)
      .unwrap()
      .parse()
      .expect("Unable to retrieve pane height");

    if self.jump {
      if !active_pane_was_in_mode {
        self.executor.execute(vec![
          "tmux".to_string(),
          "copy-mode".to_string(),
          "-t".to_string(),
          pane_id.to_string(),
        ]);
      }

      // Use a fixed filename based on pane ID so all processes use the same capture
      let capture_file = format!("/tmp/tmux-thumbs-capture-{}.txt", pane_id.replace("%", ""));
      let lock_file = format!("/tmp/tmux-thumbs-capture-{}.lock", pane_id.replace("%", ""));
      let debug_file = format!("/tmp/tmux-thumbs-debug-{}.txt", std::process::id());

      // Check if capture file exists and is recent (within 300 seconds = 5 minutes)
      let check_command = format!(
        "if [ -f {} ] && [ $(( $(date +%s) - $(stat -f %m {} 2>/dev/null || echo 0) )) -lt 300 ]; then echo 'recent'; else echo 'old'; fi",
        capture_file, capture_file
      );
      let check_result = self.executor.execute(vec!["sh".to_string(), "-c".to_string(), check_command]);
      let should_capture = check_result.trim() != "recent";

      if should_capture {
        // Need to capture - try to get lock
        let lock_command = format!("mkdir {} 2>/dev/null && echo 'locked' || echo 'exists'", lock_file);
        let lock_result = self.executor.execute(vec!["sh".to_string(), "-c".to_string(), lock_command]);

        if lock_result.trim() == "locked" {
          // We got the lock, capture the buffer
          // When in copy-mode, capture the visible window (frozen view)
          // Use -e to capture the visible area including scrollback at current position
          let capture_command = if active_pane_was_in_mode {
            // Already in copy-mode: capture what's currently visible in copy-mode view
            format!("tmux capture-pane -p -e -t {} > {}", pane_id, capture_file)
          } else {
            // Not in copy-mode yet: capture from scroll position 0
            let pane_scroll_position_val: i32 = 0;
            let scroll_params = format!("-S {} -E {}", -pane_scroll_position_val, pane_height - pane_scroll_position_val - 1);
            format!("tmux capture-pane -p -t {} {} > {}", pane_id, scroll_params, capture_file)
          };
          self.executor.execute(vec!["sh".to_string(), "-c".to_string(), capture_command]);

          // Debug: verify capture file was created
          let verify_command = format!(
            "echo 'NEW CAPTURE (was_in_mode={}, pid={}) to: {}' > {}; wc -l {} >> {}; echo 'First icmp_seq:' >> {}; grep -o 'icmp_seq=[0-9]*' {} | head -1 >> {} 2>&1 || echo 'none' >> {}; echo 'Last icmp_seq:' >> {}; grep -o 'icmp_seq=[0-9]*' {} | tail -1 >> {} 2>&1 || echo 'none' >> {}",
            active_pane_was_in_mode,
            std::process::id(),
            capture_file,
            debug_file,
            capture_file,
            debug_file,
            debug_file,
            capture_file,
            debug_file,
            debug_file,
            debug_file,
            capture_file,
            debug_file,
            debug_file
          );
          self.executor.execute(vec!["sh".to_string(), "-c".to_string(), verify_command]);
        } else {
          // Someone else is capturing, wait for it
          let wait_command = format!("for i in {{1..50}}; do [ -f {} ] && break; sleep 0.02; done", capture_file);
          self.executor.execute(vec!["sh".to_string(), "-c".to_string(), wait_command]);

          let reuse_command = format!(
            "echo 'WAITED for capture (pid={}) from: {}' > {}; wc -l {} >> {}; echo 'First icmp_seq:' >> {}; grep -o 'icmp_seq=[0-9]*' {} | head -1 >> {} 2>&1 || echo 'none' >> {}; echo 'Last icmp_seq:' >> {}; grep -o 'icmp_seq=[0-9]*' {} | tail -1 >> {} 2>&1 || echo 'none' >> {}",
            std::process::id(),
            capture_file,
            debug_file,
            capture_file,
            debug_file,
            debug_file,
            capture_file,
            debug_file,
            debug_file,
            debug_file,
            capture_file,
            debug_file,
            debug_file
          );
          self.executor.execute(vec!["sh".to_string(), "-c".to_string(), reuse_command]);
        }
      } else {
        // Reuse existing recent capture
        let reuse_command = format!(
          "echo 'REUSE CAPTURE (pid={}) from: {}' > {}; wc -l {} >> {}; echo 'First icmp_seq:' >> {}; grep -o 'icmp_seq=[0-9]*' {} | head -1 >> {} 2>&1 || echo 'none' >> {}; echo 'Last icmp_seq:' >> {}; grep -o 'icmp_seq=[0-9]*' {} | tail -1 >> {} 2>&1 || echo 'none' >> {}",
          std::process::id(),
          capture_file,
          debug_file,
          capture_file,
          debug_file,
          debug_file,
          capture_file,
          debug_file,
          debug_file,
          debug_file,
          capture_file,
          debug_file,
          debug_file
        );
        self.executor.execute(vec!["sh".to_string(), "-c".to_string(), reuse_command]);
      }

      self.active_pane_captured_content = Some(capture_file);

      self.active_pane_in_mode = true;

      let cursor = self.executor.execute(vec![
        "tmux".to_string(),
        "display-message".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "#{copy_cursor_x}:#{copy_cursor_y}".to_string(),
      ]);
      let cursor: Vec<i32> = cursor
        .trim()
        .split(':')
        .map(|value| value.parse().expect("Unable to retrieve copy cursor position"))
        .collect();
      self.active_pane_cursor = Some((*cursor.get(0).unwrap(), *cursor.get(1).unwrap()));
    }

    let pane_width = active_pane
      .get(6)
      .unwrap_or(&"0")
      .parse()
      .expect("Unable to retrieve pane width");

    self.active_pane_width = Some(pane_width);
    self.active_pane_height = Some(pane_height);

    if self.active_pane_in_mode {
      let pane_scroll_position = if active_pane_was_in_mode {
        active_pane.get(3).unwrap().to_string()
      } else {
        self.executor.execute(vec![
          "tmux".to_string(),
          "display-message".to_string(),
          "-p".to_string(),
          "-t".to_string(),
          pane_id.to_string(),
          "#{scroll_position}".to_string(),
        ])
      }
      .parse()
      .expect("Unable to retrieve pane scroll");

      self.active_pane_scroll_position = Some(pane_scroll_position);

      let history_size = self.executor.execute(vec![
        "tmux".to_string(),
        "display-message".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "#{history_size}".to_string(),
      ])
      .parse()
      .expect("Unable to retrieve history size");

      self.active_pane_history_size = Some(history_size);
    }

    let zoomed_pane = *active_pane.get(4).expect("Unable to retrieve zoom pane property") == "1";

    self.active_pane_zoomed = Some(zoomed_pane);
  }

  pub fn execute_thumbs(&mut self) {
    let options_command = vec!["tmux", "show", "-g"];
    let params: Vec<String> = options_command.iter().map(|arg| arg.to_string()).collect();
    let options = self.executor.execute(params);
    let lines: Vec<&str> = options.split('\n').collect();

    let pattern = Regex::new(r#"^@thumbs-([\w\-0-9]+)\s+"?([^"]+)"?$"#).unwrap();

    let args = lines
      .iter()
      .flat_map(|line| {
        if let Some(captures) = pattern.captures(line) {
          let name = captures.get(1).unwrap().as_str();
          let value = captures.get(2).unwrap().as_str();

          let boolean_params = vec!["reverse", "unique", "contrast"];

          if boolean_params.iter().any(|&x| x == name) {
            return vec![format!("--{}", name)];
          }

          let string_params = vec![
            "alphabet",
            "position",
            "fg-color",
            "bg-color",
            "hint-bg-color",
            "hint-fg-color",
            "select-fg-color",
            "select-bg-color",
            "multi-fg-color",
            "multi-bg-color",
          ];

          if string_params.iter().any(|&x| x == name) {
            return vec![format!("--{}", name), format!("'{}'", value)];
          }

          if name.starts_with("url-regexp") {
            return vec!["--url-regexp".to_string(), format!("'{}'", value.replace("\\\\", "\\"))];
          }

          if name.starts_with("regexp") {
            return vec!["--regexp".to_string(), format!("'{}'", value.replace("\\\\", "\\"))];
          }

          vec![]
        } else {
          vec![]
        }
      })
      .collect::<Vec<String>>();

    let active_pane_id = self.active_pane_id.as_mut().unwrap().clone();

    let scroll_params =
      if let (Some(pane_height), Some(scroll_position)) = (self.active_pane_height, self.active_pane_scroll_position) {
        format!(" -S {} -E {}", -scroll_position, pane_height - scroll_position - 1)
      } else {
        "".to_string()
      };

    let active_pane_zoomed = self.active_pane_zoomed.as_mut().unwrap().clone();
    let zoom_command = if active_pane_zoomed {
      format!("tmux resize-pane -t {} -Z;", active_pane_id)
    } else {
      "".to_string()
    };

    let jump_args = if self.jump {
      let character_arg = self
        .character
        .as_ref()
        .map(|value| format!(" --character {}", shell_quote(value)))
        .unwrap_or_else(|| "".to_string());
      let backward_arg = if self.backward { " --backward" } else { "" };
      let start_arg = self.character.as_ref().map(|_| {
        let (x, y) = self.active_pane_cursor.unwrap();
        format!(" --start-x {} --start-y {}", x, y)
      }).unwrap_or_else(|| "".to_string());
      format!(" --jump{}{}{}", character_arg, backward_arg, start_arg)
    } else {
      "".to_string()
    };

    let capture_flags = if self.jump { "-p" } else { "-J -p" };

    let pane_command = if self.jump && self.active_pane_captured_content.is_some() {
      // Use pre-captured content from temp file for jump mode to avoid race conditions with live buffer
      let capture_file = self.active_pane_captured_content.as_ref().unwrap();

      // Debug log
      let debug_file = format!("/tmp/tmux-thumbs-debug-{}.txt", std::process::id());
      let debug_command = format!("echo 'Using pre-captured file: {}' >> {}", capture_file, debug_file);
      self.executor.execute(vec!["sh".to_string(), "-c".to_string(), debug_command]);

      // Clean up lock directory after thumbs finishes, but keep capture file for reuse
      let lock_file = format!("/tmp/tmux-thumbs-capture-{}.lock",
        self.active_pane_id.as_ref().unwrap().replace("%", ""));
      format!(
        "cat {capture_file} | tail -n {height} | {dir}/target/release/thumbs --ready-signal {ready_signal} -f '%U:%P:%X:%Y:%H' -t {tmp} {args}{jump_args}; tmux swap-pane -t {active_pane_id}; {zoom_command} tmux wait-for -S {signal}; rmdir {lock_file} 2>/dev/null || true",
        capture_file = capture_file,
        lock_file = lock_file,
        height = self.active_pane_height.unwrap_or(i32::MAX),
        dir = self.dir,
        tmp = TMP_FILE,
        ready_signal = self.ready_signal,
        args = args.join(" "),
        jump_args = jump_args,
        active_pane_id = active_pane_id,
        zoom_command = zoom_command,
        signal = self.signal
      )
    } else {
      // Debug log
      let debug_file = format!("/tmp/tmux-thumbs-debug-{}.txt", std::process::id());
      let debug_command = format!("echo 'Using live capture' >> {}", debug_file);
      self.executor.execute(vec!["sh".to_string(), "-c".to_string(), debug_command]);

      format!(
        "tmux capture-pane {capture_flags} -t {active_pane_id}{scroll_params} | tail -n {height} | {dir}/target/release/thumbs --ready-signal {ready_signal} -f '%U:%P:%X:%Y:%H' -t {tmp} {args}{jump_args}; tmux swap-pane -t {active_pane_id}; {zoom_command} tmux wait-for -S {signal}",
        capture_flags = capture_flags,
        active_pane_id = active_pane_id,
        scroll_params = scroll_params,
        height = self.active_pane_height.unwrap_or(i32::MAX),
        dir = self.dir,
        tmp = TMP_FILE,
        ready_signal = self.ready_signal,
        args = args.join(" "),
        jump_args = jump_args,
        zoom_command = zoom_command,
        signal = self.signal
      )
    };

    let thumbs_command = vec![
      "tmux",
      "new-window",
      "-P",
      "-F",
      "#{pane_id}",
      "-d",
      "-n",
      "[thumbs]",
      pane_command.as_str(),
    ];

    let params: Vec<String> = thumbs_command.iter().map(|arg| arg.to_string()).collect();

    self.thumbs_pane_id = Some(self.executor.execute(params));
  }

  pub fn swap_panes(&mut self) {
    let active_pane_id = self.active_pane_id.as_mut().unwrap().clone();
    let thumbs_pane_id = self.thumbs_pane_id.as_mut().unwrap().clone();

    let swap_command = vec![
      "tmux",
      "swap-pane",
      "-d",
      "-s",
      active_pane_id.as_str(),
      "-t",
      thumbs_pane_id.as_str(),
    ];

    let params = swap_command
      .iter()
      .filter(|&s| !s.is_empty())
      .map(|arg| arg.to_string())
      .collect();

    self.executor.execute(params);
  }

  pub fn resize_thumbs_window(&mut self) {
    let thumbs_pane_id = self.thumbs_pane_id.as_ref().unwrap();
    let width = self.active_pane_width.unwrap();
    let height = self.active_pane_height.unwrap();
    let resize_command = vec![
      "tmux".to_string(),
      "resize-window".to_string(),
      "-t".to_string(),
      thumbs_pane_id.to_string(),
      "-x".to_string(),
      width.to_string(),
      "-y".to_string(),
      height.to_string(),
    ];

    self.executor.execute(resize_command);
  }

  pub fn resize_pane(&mut self) {
    let active_pane_zoomed = self.active_pane_zoomed.as_mut().unwrap().clone();

    if !active_pane_zoomed {
      return;
    }

    let thumbs_pane_id = self.thumbs_pane_id.as_mut().unwrap().clone();

    let resize_command = vec!["tmux", "resize-pane", "-t", thumbs_pane_id.as_str(), "-Z"];

    let params = resize_command
      .iter()
      .filter(|&s| !s.is_empty())
      .map(|arg| arg.to_string())
      .collect();

    self.executor.execute(params);
  }

  pub fn wait_ready(&mut self) {
    let wait_command = vec!["tmux", "wait-for", self.ready_signal.as_str()];
    let params = wait_command.iter().map(|arg| arg.to_string()).collect();

    self.executor.execute(params);
  }

  pub fn wait_thumbs(&mut self) {
    let wait_command = vec!["tmux", "wait-for", self.signal.as_str()];
    let params = wait_command.iter().map(|arg| arg.to_string()).collect();

    self.executor.execute(params);
  }

  pub fn retrieve_content(&mut self) {
    let retrieve_command = vec!["cat", TMP_FILE];
    let params = retrieve_command.iter().map(|arg| arg.to_string()).collect();

    self.content = Some(self.executor.execute(params));
  }

  pub fn destroy_content(&mut self) {
    let retrieve_command = vec!["rm", TMP_FILE];
    let params = retrieve_command.iter().map(|arg| arg.to_string()).collect();

    self.executor.execute(params);
  }

  pub fn send_osc52(&mut self) {}

  fn is_url_pattern(pattern: &str) -> bool {
    pattern == "url" || pattern == "markdown_url" || pattern == "url_custom"
  }

  fn command_for_selection<'b>(
    upcase: bool,
    pattern: &str,
    command: &'b str,
    upcase_command: &'b str,
    url_command: &'b str,
  ) -> &'b str {
    if upcase && Self::is_url_pattern(pattern) {
      url_command
    } else if upcase {
      upcase_command
    } else {
      command
    }
  }

  fn cursor_delta(from: i32, to: i32, positive: &'static str, negative: &'static str) -> Option<(&'static str, i32)> {
    if to > from {
      Some((positive, to - from))
    } else if to < from {
      Some((negative, from - to))
    } else {
      None
    }
  }

  fn shell_cursor_command(pane_id: &str, count: i32, direction: &str) -> Vec<String> {
    vec![
      "bash".to_string(),
      "-c".to_string(),
      "tmux send-keys -t \"$1\" -X -N \"$2\" \"$3\" || for ((i=0; i<$2; i++)); do tmux send-keys -t \"$1\" -X \"$3\"; done".to_string(),
      "--".to_string(),
      pane_id.to_string(),
      count.to_string(),
      direction.to_string(),
    ]
  }

  fn start_of_line_command(pane_id: &str) -> Vec<String> {
    vec![
      "tmux".to_string(),
      "send-keys".to_string(),
      "-t".to_string(),
      pane_id.to_string(),
      "-X".to_string(),
      "start-of-line".to_string(),
    ]
  }

  fn move_cursor_to(&mut self, x: i32, y: i32) {
    let pane_id = self.active_pane_id.clone().unwrap();
    let captured_scroll = self.active_pane_scroll_position.unwrap_or(0);

    // Read current scroll position to detect if the view has shifted
    let current_scroll_str = self.executor.execute(vec![
      "tmux".to_string(),
      "display-message".to_string(),
      "-p".to_string(),
      "-t".to_string(),
      pane_id.clone(),
      "#{scroll_position}".to_string(),
    ]);
    let current_scroll: i32 = current_scroll_str.trim().parse().unwrap_or(0);

    // If scroll position changed, the view has auto-scrolled due to buffer growth.
    // When buffer grows and fills the pane, tmux auto-scrolls up (scroll_position increases).
    // Our captured coordinates are relative to scroll_position at capture time.
    // We need to adjust by the scroll delta.
    let scroll_delta = current_scroll - captured_scroll;

    // Since we're using pre-captured content, the y coordinate from thumbs
    // corresponds to the frozen view at captured_scroll position.
    // If scroll_position increased, the view shifted up, so we need to move down.
    let adjusted_y = y + scroll_delta;

    // Move cursor to top of screen (y=0)
    self.executor.execute(vec![
      "tmux".to_string(),
      "send-keys".to_string(),
      "-t".to_string(),
      pane_id.clone(),
      "-X".to_string(),
      "top-line".to_string(),
    ]);

    // Move to adjusted target row
    if adjusted_y > 0 {
      self.executor.execute(Self::shell_cursor_command(&pane_id, adjusted_y, "cursor-down"));
    } else if adjusted_y < 0 {
      self.executor.execute(Self::shell_cursor_command(&pane_id, -adjusted_y, "cursor-up"));
    }

    // Move to start of line
    self.executor.execute(Self::start_of_line_command(&pane_id));

    // Move to target column
    if x > 0 {
      self.executor.execute(Self::shell_cursor_command(&pane_id, x, "cursor-right"));
    }
  }

  pub fn execute_command(&mut self) {
    let content = self.content.clone().unwrap();
    if content.trim().is_empty() {
      return;
    }
    let items: Vec<&str> = content.split('\n').collect();

    if self.jump {
      let item = items.first().unwrap();
      let mut splitter = item.splitn(5, ':');
      splitter.next();
      splitter.next();
      let x = splitter.next().unwrap().parse().expect("Invalid jump column");
      let y = splitter.next().unwrap().parse().expect("Invalid jump row");
      self.move_cursor_to(x, y);
      return;
    }

    if items.len() > 1 {
      let text = items
        .iter()
        .map(|item| item.splitn(5, ':').last().unwrap())
        .collect::<Vec<&str>>()
        .join(" ");

      self.execute_final_command(&text, &self.multi_command.clone());

      return;
    }

    // Only one item
    let item: &str = items.first().unwrap();

    let mut splitter = item.splitn(5, ':');

    if let Some(upcase) = splitter.next() {
      if let Some(pattern) = splitter.next() {
        if let Some(_x) = splitter.next() {
          if let Some(_y) = splitter.next() {
            if let Some(text) = splitter.next() {
          if self.osc52 {
            let base64_text = base64::encode(text.as_bytes());
            let osc_seq = format!("\x1b]52;0;{}\x07", base64_text);
            let tmux_seq = format!("\x1bPtmux;{}\x1b\\", osc_seq.replace("\x1b", "\x1b\x1b"));

            // FIXME: Review if this comment is still rellevant
            //
            // When the user selects a match:
            // 1. The `rustbox` object created in the `viewbox` above is dropped.
            // 2. During its `drop`, the `rustbox` object sends a CSI 1049 escape
            //    sequence to tmux.
            // 3. This escape sequence causes the `window_pane_alternate_off` function
            //    in tmux to be called.
            // 4. In `window_pane_alternate_off`, tmux sets the needs-redraw flag in the
            //    pane.
            // 5. If we print the OSC copy escape sequence before the redraw is completed,
            //    tmux will *not* send the sequence to the host terminal. See the following
            //    call chain in tmux: `input_dcs_dispatch` -> `screen_write_rawstring`
            //    -> `tty_write` -> `tty_client_ready`. In this case, `tty_client_ready`
            //    will return false, thus preventing the sequence from being sent.
            //
            // Therefore, for now we wait a little bit here for the redraw to finish.
            std::thread::sleep(std::time::Duration::from_millis(100));

            std::io::stdout().write_all(tmux_seq.as_bytes()).unwrap();
            std::io::stdout().flush().unwrap();
          }

          let execute_command = Self::command_for_selection(
            upcase.trim_end() == "true",
            pattern,
            &self.command,
            &self.upcase_command,
            &self.url_command,
          )
          .to_string();

          // The command we run has two arguments:
          //  * The first arg is the (trimmed) text. This gets stored in a variable, in order to
          //    preserve quoting and special characters.
          //
          //  * The second argument is the user's command, with the '{}' token replaced with an
          //    unquoted reference to the variable containing the text.
          //
          // The reference is unquoted, unfortunately, because the token may already have been
          // spliced into a string (e.g 'tmux display-message "Copied {}"'), and it's impossible (or
          // at least exceedingly difficult) to determine the correct quoting level.
          //
          // The alternative of literally splicing the text into the command is bad and it causes all
          // kinds of harmful escaping issues that the user cannot reasonable avoid.
          //
          // For example, imagine some pattern matched the text "foo;rm *" and the user's command was
          // an innocuous "echo {}". With literal splicing, we would run the command "echo foo;rm *".
          // That's BAD. Without splicing, instead we execute "echo ${THUMB}" which does mostly the
          // right thing regardless the contents of the text. (At worst, bash will word-separate the
          // unquoted variable; but it won't _execute_ those words in common scenarios).
          //
          // Ideally user commands would just use "${THUMB}" to begin with rather than having any
          // sort of ad-hoc string splicing here at all, and then they could specify the quoting they
          // want, but that would break backwards compatibility.
              self.execute_final_command(text.trim_end(), &execute_command);
            }
          }
        }
      }
    }
  }

  pub fn execute_final_command(&mut self, text: &str, execute_command: &str) {
    let final_command = str::replace(execute_command, "{}", "${THUMB}");
    let retrieve_command = vec![
      "bash",
      "-c",
      "THUMB=\"$1\"; eval \"$2\"",
      "--",
      text,
      final_command.as_str(),
    ];

    let params = retrieve_command.iter().map(|arg| arg.to_string()).collect();

    self.executor.execute(params);
  }
}

fn default_url_command() -> &'static str {
  if cfg!(target_os = "macos") {
    "tmux set-buffer -- \"{}\" && open \"{}\""
  } else {
    "tmux set-buffer -- \"{}\""
  }
}

fn shell_quote(value: &str) -> String {
  format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
  use super::*;

  struct TestShell {
    outputs: Vec<String>,
    executed: Vec<Vec<String>>,
  }

  impl TestShell {
    fn new(outputs: Vec<String>) -> TestShell {
      TestShell {
        executed: vec![],
        outputs,
      }
    }
  }

  impl Executor for TestShell {
    fn execute(&mut self, args: Vec<String>) -> String {
      self.executed.push(args);
      self.outputs.pop().unwrap()
    }

    fn last_executed(&self) -> Option<Vec<String>> {
      self.executed.last().cloned()
    }
  }

  #[test]
  fn retrieve_active_pane() {
    let last_command_outputs = vec!["%97:100:24:1:0:active\n%106:100:24:1:0:nope\n%107:100:24:1:0:nope\n".to_string()];
    let mut executor = TestShell::new(last_command_outputs);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      false,
      None,
      false,
      false,
    );

    swapper.capture_active_pane();

    assert_eq!(swapper.active_pane_id.unwrap(), "%97");
  }

  #[test]
  fn jump_mode_enters_copy_mode_from_a_live_pane() {
    let outputs = vec![
      "0".to_string(),
      "3:4".to_string(),
      "".to_string(),
      "%97:0:24::0:active\n".to_string(),
    ];
    let mut executor = TestShell::new(outputs);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      true,
      Some("a".to_string()),
      false,
      false,
    );

    swapper.capture_active_pane();

    assert!(swapper.active_pane_in_mode);
    assert_eq!(swapper.active_pane_cursor, Some((3, 4)));
    assert_eq!(swapper.active_pane_scroll_position, Some(0));
  }

  #[test]
  fn swap_panes() {
    let last_command_outputs = vec![
      "".to_string(),
      "%100".to_string(),
      "".to_string(),
      "%106:100:24:1:0:nope\n%98:100:24:1:0:active\n%107:100:24:1:0:nope\n".to_string(),
    ];
    let mut executor = TestShell::new(last_command_outputs);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      false,
      None,
      false,
      false,
    );

    swapper.capture_active_pane();
    swapper.execute_thumbs();
    swapper.swap_panes();

    let expectation = vec!["tmux", "swap-pane", "-d", "-s", "%98", "-t", "%100"];

    assert_eq!(executor.last_executed().unwrap(), expectation);
  }

  #[test]
  fn resize_thumbs_window_matches_source_pane_geometry() {
    let mut executor = TestShell::new(vec!["".to_string()]);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      true,
      Some("A".to_string()),
      false,
      false,
    );
    swapper.thumbs_pane_id = Some("%2".to_string());
    swapper.active_pane_width = Some(80);
    swapper.active_pane_height = Some(15);

    swapper.resize_thumbs_window();

    assert_eq!(
      executor.last_executed().unwrap(),
      vec!["tmux", "resize-window", "-t", "%2", "-x", "80", "-y", "15"]
    );
  }

  #[test]
  fn jump_capture_preserves_wrapped_screen_rows() {
    let mut executor = TestShell::new(vec!["%100".to_string(), "".to_string()]);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "/tmp/thumbs".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      true,
      Some("A".to_string()),
      false,
      false,
    );
    swapper.active_pane_id = Some("%1".to_string());
    swapper.active_pane_cursor = Some((0, 0));
    swapper.active_pane_height = Some(5);
    swapper.active_pane_zoomed = Some(false);

    swapper.execute_thumbs();

    let command = executor.executed.get(1).unwrap().get(8).unwrap();
    assert!(command.contains("capture-pane -p"));
    assert!(!command.contains("capture-pane -J"));
  }

  #[test]
  fn quoted_execution() {
    let last_command_outputs = vec!["Blah blah blah, the ignored user script output".to_string()];
    let mut executor = TestShell::new(last_command_outputs);

    let user_command = "echo \"{}\"".to_string();
    let upcase_command = "open \"{}\"".to_string();
    let multi_command = "open \"{}\"".to_string();
    let url_command = "open \"{}\"".to_string();
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      user_command,
      upcase_command,
      multi_command,
      url_command,
      false,
      None,
      false,
      false,
    );

    swapper.content = Some(format!(
      "{do_upcase}:path:0:0:{thumb_text}",
      do_upcase = false,
      thumb_text = "foobar;rm *",
    ));
    swapper.execute_command();

    let expectation = vec![
      "bash",
      // The actual shell command:
      "-c",
      "THUMB=\"$1\"; eval \"$2\"",
      // $0: The non-existent program name.
      "--",
      // $1: The value assigned to THUMB above.
      //     Not interpreted as a shell expression!
      "foobar;rm *",
      // $2: The user script, with {} replaced with ${THUMB},
      //     and will be eval'd with THUMB in scope.
      "echo \"${THUMB}\"",
    ];

    assert_eq!(executor.last_executed().unwrap(), expectation);
  }

  #[test]
  fn uppercase_url_uses_url_command() {
    assert_eq!(
      Swapper::command_for_selection(true, "url", "copy", "upcase", "copy-and-open"),
      "copy-and-open"
    );
    assert_eq!(
      Swapper::command_for_selection(true, "markdown_url", "copy", "upcase", "copy-and-open"),
      "copy-and-open"
    );
    assert_eq!(
      Swapper::command_for_selection(true, "url_custom", "copy", "upcase", "copy-and-open"),
      "copy-and-open"
    );
  }

  #[test]
  fn uppercase_url_copies_and_opens_with_quoted_value() {
    let mut executor = TestShell::new(vec!["".to_string()]);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      "copy \"{}\"".to_string(),
      "upcase \"{}\"".to_string(),
      "multi \"{}\"".to_string(),
      "tmux set-buffer -- \"{}\" && open \"{}\"".to_string(),
      false,
      None,
      false,
      false,
    );

    swapper.content = Some("true:url:0:0:https://example.com/a;not-a-command".to_string());
    swapper.execute_command();

    let expectation = vec![
      "bash",
      "-c",
      "THUMB=\"$1\"; eval \"$2\"",
      "--",
      "https://example.com/a;not-a-command",
      "tmux set-buffer -- \"${THUMB}\" && open \"${THUMB}\"",
    ];

    assert_eq!(executor.last_executed().unwrap(), expectation);
  }

  #[test]
  fn non_url_selection_preserves_copy_commands() {
    assert_eq!(
      Swapper::command_for_selection(false, "url", "copy", "upcase", "copy-and-open"),
      "copy"
    );
    assert_eq!(
      Swapper::command_for_selection(true, "path", "copy", "upcase", "copy-and-open"),
      "upcase"
    );
  }

  #[test]
  fn cursor_delta_selects_direction_and_count() {
    assert_eq!(
      Swapper::cursor_delta(2, 5, "cursor-down", "cursor-up"),
      Some(("cursor-down", 3))
    );
    assert_eq!(
      Swapper::cursor_delta(5, 2, "cursor-right", "cursor-left"),
      Some(("cursor-left", 3))
    );
    assert_eq!(Swapper::cursor_delta(4, 4, "positive", "negative"), None);
  }

  #[test]
  fn jump_selection_starts_the_line_then_moves_logical_columns() {
    let mut executor = TestShell::new(vec!["".to_string(), "".to_string(), "".to_string(), "0:0:0".to_string()]);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      "copy {}".to_string(),
      "upcase {}".to_string(),
      "multi {}".to_string(),
      "url {}".to_string(),
      true,
      Some("x".to_string()),
      false,
      false,
    );
    swapper.active_pane_id = Some("%1".to_string());
    swapper.active_pane_cursor = Some((0, 0));
    swapper.active_pane_scroll_position = Some(0);
    swapper.content = Some("false:character:12:2:A".to_string());

    swapper.execute_command();

    assert_eq!(executor.executed.len(), 4);
    assert_eq!(
      executor.executed.get(0).unwrap().get(5).unwrap(),
      "#{copy_cursor_x}:#{copy_cursor_y}:#{scroll_position}"
    );
    assert_eq!(executor.executed.get(1).unwrap().get(5).unwrap(), "2");
    assert_eq!(executor.executed.get(1).unwrap().get(6).unwrap(), "cursor-down");
    assert_eq!(
      executor.executed.get(2).unwrap(),
      &vec!["tmux", "send-keys", "-t", "%1", "-X", "start-of-line"]
    );
    assert_eq!(executor.executed.get(3).unwrap().get(5).unwrap(), "12");
    assert_eq!(executor.executed.get(3).unwrap().get(6).unwrap(), "cursor-right");
  }

  #[test]
  fn jump_movement_reanchors_after_pane_resize() {
    let mut executor = TestShell::new(vec!["".to_string(), "".to_string(), "2:1:0".to_string()]);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      true,
      Some("A".to_string()),
      false,
      false,
    );
    swapper.active_pane_id = Some("%1".to_string());
    swapper.active_pane_cursor = Some((2, 14));
    swapper.active_pane_scroll_position = Some(13);
    swapper.content = Some("false:character:12:14:A".to_string());

    swapper.execute_command();

    assert_eq!(executor.executed.len(), 3);
    assert_eq!(
      executor.executed.get(0).unwrap().get(5).unwrap(),
      "#{copy_cursor_x}:#{copy_cursor_y}:#{scroll_position}"
    );
    assert_eq!(executor.executed.get(1).unwrap().get(5).unwrap(), "start-of-line");
    assert_eq!(executor.executed.get(2).unwrap().get(5).unwrap(), "12");
  }

  #[test]
  fn empty_jump_selection_does_not_panic_during_cleanup() {
    let mut executor = TestShell::new(vec![]);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      "".to_string(),
      true,
      Some("A".to_string()),
      false,
      false,
    );
    swapper.content = Some("".to_string());

    swapper.execute_command();

    assert!(executor.executed.is_empty());
  }

  #[test]
  fn multi_selection_uses_multi_command() {
    let mut executor = TestShell::new(vec!["".to_string()]);
    let mut swapper = Swapper::new(
      Box::new(&mut executor),
      "".to_string(),
      "copy {}".to_string(),
      "upcase {}".to_string(),
      "multi {}".to_string(),
      "copy-and-open {}".to_string(),
      false,
      None,
      false,
      false,
    );

    swapper.content = Some("false:url:0:0:https://example.com\nfalse:path:0:0:/tmp/example".to_string());
    swapper.execute_command();

    let expectation = vec![
      "bash",
      "-c",
      "THUMB=\"$1\"; eval \"$2\"",
      "--",
      "https://example.com /tmp/example",
      "multi ${THUMB}",
    ];

    assert_eq!(executor.last_executed().unwrap(), expectation);
  }
}

fn app_args<'a>() -> clap::ArgMatches<'a> {
  App::new("tmux-thumbs")
    .version(crate_version!())
    .about("A lightning fast version of tmux-fingers, copy/pasting tmux like vimium/vimperator")
    .arg(
      Arg::with_name("dir")
        .help("Directory where to execute thumbs")
        .long("dir")
        .default_value(""),
    )
    .arg(
      Arg::with_name("command")
        .help("Command to execute after choose a hint")
        .long("command")
        .default_value("tmux set-buffer -- \"{}\" && tmux display-message \"Copied {}\""),
    )
    .arg(
      Arg::with_name("upcase_command")
        .help("Command to execute after choose a hint, in upcase")
        .long("upcase-command")
        .default_value("tmux set-buffer -- \"{}\" && tmux paste-buffer && tmux display-message \"Copied {}\""),
    )
    .arg(
      Arg::with_name("multi_command")
        .help("Command to execute after choose multiple hints")
        .long("multi-command")
        .default_value("tmux set-buffer -- \"{}\" && tmux paste-buffer && tmux display-message \"Multi copied {}\""),
    )
    .arg(
      Arg::with_name("url_command")
        .help("Command to execute for an uppercase URL hint")
        .long("url-command")
        .default_value(default_url_command()),
    )
    .arg(
      Arg::with_name("jump")
        .help("Move the copy-mode cursor to the selected match")
        .long("jump"),
    )
    .arg(
      Arg::with_name("character")
        .help("Pass a character search to thumbs")
        .long("character")
        .takes_value(true),
    )
    .arg(
      Arg::with_name("backward")
        .help("Search character matches in reverse order")
        .long("backward"),
    )
    .arg(
      Arg::with_name("osc52")
        .help("Print OSC52 copy escape sequence in addition to running the pick command")
        .long("osc52")
        .short("o"),
    )
    .get_matches()
}

fn main() -> std::io::Result<()> {
  let args = app_args();
  let dir = args.value_of("dir").unwrap();
  let command = args.value_of("command").unwrap();
  let upcase_command = args.value_of("upcase_command").unwrap();
  let multi_command = args.value_of("multi_command").unwrap();
  let url_command = args.value_of("url_command").unwrap();
  let jump = args.is_present("jump");
  let character = args.value_of("character").map(|value| value.to_string());
  let backward = args.is_present("backward");
  let osc52 = args.is_present("osc52");

  if dir.is_empty() {
    panic!("Invalid tmux-thumbs execution. Are you trying to execute tmux-thumbs directly?")
  }

  // Debug: log process start
  let debug_file = format!("/tmp/tmux-thumbs-main-debug-{}.txt", std::process::id());
  std::fs::write(&debug_file, format!("Process {} started with jump={} character={:?}\n", std::process::id(), jump, character)).ok();

  let mut executor = RealShell::new();
  let mut swapper = Swapper::new(
    Box::new(&mut executor),
    dir.to_string(),
    command.to_string(),
    upcase_command.to_string(),
    multi_command.to_string(),
    url_command.to_string(),
    jump,
    character,
    backward,
    osc52,
  );

  swapper.capture_active_pane();
  swapper.execute_thumbs();
  swapper.resize_thumbs_window();
  // Keep the original pane visible until the hidden UI has painted its first frame.
  swapper.wait_ready();
  swapper.swap_panes();
  swapper.resize_pane();
  swapper.wait_thumbs();
  swapper.retrieve_content();
  swapper.destroy_content();
  swapper.execute_command();

  Ok(())
}
