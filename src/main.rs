use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde::Deserialize;
use wana_kana::ConvertJapanese;

const API_BASE: &str = "https://api.wanikani.com/v2";

type AppResult<T> = Result<T, Box<dyn Error>>;

fn main() -> AppResult<()> {
    let terminal = init_terminal()?;
    let result = TuiApp::new().run(terminal);
    restore_terminal()?;
    result
}

fn init_terminal() -> AppResult<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    Ok(Terminal::new(backend)?)
}

fn restore_terminal() -> AppResult<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Login,
    Menu,
    Limit(SessionMode),
    LessonStudy,
    LessonQuiz,
    ReviewQuiz,
    Done(SessionMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionMode {
    Lessons,
    Reviews,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Question {
    Meaning,
    Reading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedbackAction {
    ContinueQuestion,
    NextLessonQuestion,
    NextLesson,
    NextReviewQuestion,
    SubmitReview,
}

#[derive(Debug, Clone)]
struct Feedback {
    correct: bool,
    typo_accepted: bool,
    typed: String,
    expected: Vec<String>,
    action: FeedbackAction,
    question: Question,
}

struct TuiApp {
    client: Option<WaniKaniClient>,
    username: String,
    summary: Option<SummaryResponse>,
    screen: Screen,
    token_input: String,
    limit_input: String,
    answer_input: String,
    error: Option<String>,
    status: Option<String>,
    subjects: Vec<ApiResource<SubjectData>>,
    assignments_by_subject: HashMap<u64, u64>,
    subject_index: usize,
    lesson_study_index: usize,
    question: Question,
    feedback: Option<Feedback>,
    incorrect_meaning_answers: u32,
    incorrect_reading_answers: u32,
    correct_answers: usize,
    total_answers: usize,
    should_quit: bool,
}

impl TuiApp {
    fn new() -> Self {
        let stored_token = load_stored_token();
        let status = stored_token
            .as_ref()
            .map(|_| "Press Enter to sign in with the saved token.".to_string());
        Self {
            client: None,
            username: String::new(),
            summary: None,
            screen: Screen::Login,
            token_input: stored_token.unwrap_or_default(),
            limit_input: String::new(),
            answer_input: String::new(),
            error: None,
            status,
            subjects: Vec::new(),
            assignments_by_subject: HashMap::new(),
            subject_index: 0,
            lesson_study_index: 0,
            question: Question::Meaning,
            feedback: None,
            incorrect_meaning_answers: 0,
            incorrect_reading_answers: 0,
            correct_answers: 0,
            total_answers: 0,
            should_quit: false,
        }
    }

    fn run(mut self, mut terminal: Terminal<CrosstermBackend<Stdout>>) -> AppResult<()> {
        loop {
            terminal.draw(|frame| self.render(frame))?;
            if self.should_quit {
                return Ok(());
            }

            if event::poll(Duration::from_millis(250))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
                    Event::Paste(text) => self.handle_paste(text),
                    _ => {}
                }
            }
        }
    }

    fn render(&self, frame: &mut Frame) {
        match self.screen {
            Screen::Login => self.render_login(frame),
            Screen::Menu => self.render_menu(frame),
            Screen::Limit(mode) => self.render_limit(frame, mode),
            Screen::LessonStudy => self.render_lesson_study(frame),
            Screen::LessonQuiz => self.render_quiz(frame, SessionMode::Lessons),
            Screen::ReviewQuiz => self.render_quiz(frame, SessionMode::Reviews),
            Screen::Done(mode) => self.render_done(frame, mode),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        match self.screen {
            Screen::Login => self.handle_login_key(key),
            Screen::Menu => self.handle_menu_key(key),
            Screen::Limit(mode) => self.handle_limit_key(key, mode),
            Screen::LessonStudy => self.handle_lesson_study_key(key),
            Screen::LessonQuiz => self.handle_lesson_quiz_key(key),
            Screen::ReviewQuiz => self.handle_review_quiz_key(key),
            Screen::Done(_) => self.handle_done_key(key),
        }
    }

    fn handle_paste(&mut self, text: String) {
        match self.screen {
            Screen::Login => {
                self.token_input.push_str(text.trim());
                self.error = None;
            }
            Screen::Limit(_) => {
                self.limit_input.push_str(
                    &text
                        .chars()
                        .filter(char::is_ascii_digit)
                        .collect::<String>(),
                );
                self.error = None;
            }
            Screen::LessonQuiz | Screen::ReviewQuiz if self.feedback.is_none() => {
                self.answer_input.push_str(text.trim());
            }
            _ => {}
        }
    }

    fn handle_login_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Enter => self.login(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.token_input.clear();
                self.error = None;
            }
            KeyCode::Delete => {
                self.token_input.clear();
                self.error = None;
            }
            KeyCode::Backspace => {
                self.token_input.pop();
                self.error = None;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.token_input.push(character);
                self.error = None;
            }
            _ => {}
        }
    }

    fn handle_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('t') => {
                if let Err(err) = delete_stored_token() {
                    self.error = Some(format!("Could not delete saved token: {err}"));
                    return;
                }
                self.client = None;
                self.summary = None;
                self.username.clear();
                self.token_input.clear();
                self.error = None;
                self.status = Some("Logged out. Saved token removed.".to_string());
                self.screen = Screen::Login;
            }
            KeyCode::Char('r') => self.open_limit_prompt(SessionMode::Reviews),
            KeyCode::Char('l') => self.open_limit_prompt(SessionMode::Lessons),
            _ => {}
        }
    }

    fn handle_limit_key(&mut self, key: KeyEvent, mode: SessionMode) {
        match key.code {
            KeyCode::Esc => {
                self.error = None;
                self.screen = Screen::Menu;
            }
            KeyCode::Enter => self.start_session(mode),
            KeyCode::Backspace => {
                self.limit_input.pop();
            }
            KeyCode::Char(character) if character.is_ascii_digit() => {
                self.limit_input.push(character);
                self.error = None;
            }
            _ => {}
        }
    }

    fn handle_lesson_study_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.screen = Screen::Menu,
            KeyCode::Right | KeyCode::Char('n') => self.move_lesson_study(1),
            KeyCode::Left | KeyCode::Char('p') => self.move_lesson_study(-1),
            KeyCode::Enter => {
                self.subject_index = 0;
                self.question = Question::Meaning;
                self.answer_input.clear();
                self.feedback = None;
                self.screen = Screen::LessonQuiz;
            }
            _ => {}
        }
    }

    fn move_lesson_study(&mut self, delta: isize) {
        if self.subjects.is_empty() {
            return;
        }

        let last_index = self.subjects.len() - 1;
        let next_index = self
            .lesson_study_index
            .saturating_add_signed(delta)
            .min(last_index);

        if next_index != self.lesson_study_index {
            self.lesson_study_index = next_index;
            self.question = Question::Meaning;
            self.answer_input.clear();
            self.feedback = None;
            self.error = None;
            self.status = None;
        }
    }

    fn handle_lesson_quiz_key(&mut self, key: KeyEvent) {
        if self.feedback.is_some() {
            match key.code {
                KeyCode::Enter => self.resolve_lesson_feedback(),
                KeyCode::Char('a') => self.accept_feedback_answer(false),
                KeyCode::Char('s') => self.accept_feedback_answer(true),
                _ => {}
            }
            return;
        }

        self.handle_answer_key(key, SessionMode::Lessons);
    }

    fn handle_review_quiz_key(&mut self, key: KeyEvent) {
        if self.feedback.is_some() {
            match key.code {
                KeyCode::Enter => self.resolve_review_feedback(),
                KeyCode::Char('a') => self.accept_feedback_answer(false),
                KeyCode::Char('s') => self.accept_feedback_answer(true),
                _ => {}
            }
            return;
        }

        self.handle_answer_key(key, SessionMode::Reviews);
    }

    fn handle_done_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Enter => self.screen = Screen::Menu,
            _ => {}
        }
    }

    fn handle_answer_key(&mut self, key: KeyEvent, mode: SessionMode) {
        match key.code {
            KeyCode::Esc => self.screen = Screen::Menu,
            KeyCode::Enter => self.grade_answer(mode),
            KeyCode::Backspace => {
                self.answer_input.pop();
            }
            KeyCode::Char(character) => self.answer_input.push(character),
            _ => {}
        }
    }

    fn login(&mut self) {
        let token = self.token_input.trim().to_string();
        if token.is_empty() {
            self.error = Some("Enter a WaniKani API v2 token.".to_string());
            return;
        }

        let client = WaniKaniClient::new(token);
        match (client.user(), client.summary()) {
            (Ok(user), Ok(summary)) => {
                let save_status = save_stored_token(self.token_input.trim())
                    .err()
                    .map(|err| format!("Signed in, but could not save token: {err}"));
                self.username = user.data.username;
                self.summary = Some(summary);
                self.client = Some(client);
                self.error = None;
                self.status = save_status;
                self.screen = Screen::Menu;
            }
            (Err(err), _) | (_, Err(err)) => {
                self.error = Some(format!("Login failed: {err}"));
                self.token_input.clear();
            }
        }
    }

    fn open_limit_prompt(&mut self, mode: SessionMode) {
        let available = self.available_count(mode);
        if available == 0 {
            self.error = Some(format!(
                "No {} are available right now.",
                mode.label_lower()
            ));
            return;
        }

        self.error = None;
        self.status = None;
        self.limit_input = available.min(10).to_string();
        self.screen = Screen::Limit(mode);
    }

    fn start_session(&mut self, mode: SessionMode) {
        let available = self.available_count(mode);
        let limit = self
            .limit_input
            .parse::<usize>()
            .unwrap_or(available.min(10))
            .clamp(1, available);

        let Some(client) = self.client.as_ref() else {
            self.screen = Screen::Login;
            return;
        };
        let Some(summary) = self.summary.as_ref() else {
            self.error = Some("Summary is not loaded.".to_string());
            return;
        };

        let buckets = match mode {
            SessionMode::Lessons => &summary.data.lessons,
            SessionMode::Reviews => &summary.data.reviews,
        };
        let ids = collect_available_subject_ids(buckets, limit, Utc::now());

        match (client.subjects(&ids), client.assignments_for_subjects(&ids)) {
            (Ok(subjects), Ok(assignments)) => {
                self.subjects = subjects;
                self.assignments_by_subject = assignment_map(assignments);
                self.subject_index = 0;
                self.lesson_study_index = 0;
                self.question = Question::Meaning;
                self.answer_input.clear();
                self.feedback = None;
                self.incorrect_meaning_answers = 0;
                self.incorrect_reading_answers = 0;
                self.correct_answers = 0;
                self.total_answers = 0;
                self.error = None;
                self.status = None;
                self.screen = match mode {
                    SessionMode::Lessons => Screen::LessonStudy,
                    SessionMode::Reviews => Screen::ReviewQuiz,
                };
            }
            (Err(err), _) | (_, Err(err)) => {
                self.error = Some(format!("Could not start {}: {err}", mode.label_lower()));
                self.screen = Screen::Menu;
            }
        }
    }

    fn grade_answer(&mut self, mode: SessionMode) {
        let Some(subject) = self.current_subject() else {
            return;
        };

        let has_reading_question = subject.has_reading_question();
        let expected = match self.question {
            Question::Meaning => subject.accepted_meanings(),
            Question::Reading => subject.accepted_readings(),
        };
        let typed = match self.question {
            Question::Meaning => self.answer_input.trim().to_string(),
            Question::Reading => self.answer_input.trim().to_hiragana(),
        };
        let grade = match self.question {
            Question::Meaning => grade_meaning_answer(&typed, &expected),
            Question::Reading => {
                if matches_answer(&typed, &expected) {
                    AnswerGrade::Correct
                } else {
                    AnswerGrade::Incorrect
                }
            }
        };
        let correct = grade.is_accepted();

        self.total_answers += 1;
        if correct {
            self.correct_answers += 1;
        }

        let action = match mode {
            SessionMode::Lessons => {
                if !correct {
                    FeedbackAction::ContinueQuestion
                } else if self.question == Question::Meaning && has_reading_question {
                    FeedbackAction::NextLessonQuestion
                } else {
                    FeedbackAction::NextLesson
                }
            }
            SessionMode::Reviews => {
                if self.question == Question::Meaning {
                    if !correct {
                        self.incorrect_meaning_answers += 1;
                    }
                    if has_reading_question {
                        FeedbackAction::NextReviewQuestion
                    } else {
                        FeedbackAction::SubmitReview
                    }
                } else {
                    if !correct {
                        self.incorrect_reading_answers += 1;
                    }
                    FeedbackAction::SubmitReview
                }
            }
        };

        self.feedback = Some(Feedback {
            correct,
            typo_accepted: grade == AnswerGrade::Typo,
            typed,
            expected,
            action,
            question: self.question,
        });
    }

    fn resolve_lesson_feedback(&mut self) {
        let Some(feedback) = self.feedback.take() else {
            return;
        };

        self.answer_input.clear();
        match feedback.action {
            FeedbackAction::ContinueQuestion => {}
            FeedbackAction::NextLessonQuestion => self.question = Question::Reading,
            FeedbackAction::NextLesson => {
                self.start_current_assignment();
                self.subject_index += 1;
                self.question = Question::Meaning;
                if self.subject_index >= self.subjects.len() {
                    self.screen = Screen::Done(SessionMode::Lessons);
                } else {
                    self.screen = Screen::LessonQuiz;
                }
            }
            _ => {}
        }
    }

    fn resolve_review_feedback(&mut self) {
        let Some(feedback) = self.feedback.take() else {
            return;
        };

        self.answer_input.clear();
        match feedback.action {
            FeedbackAction::NextReviewQuestion => self.question = Question::Reading,
            FeedbackAction::SubmitReview => {
                self.submit_current_review();
                self.subject_index += 1;
                self.question = Question::Meaning;
                self.incorrect_meaning_answers = 0;
                self.incorrect_reading_answers = 0;
                if self.subject_index >= self.subjects.len() {
                    self.screen = Screen::Done(SessionMode::Reviews);
                }
            }
            _ => {}
        }
    }

    fn accept_feedback_answer(&mut self, add_synonym: bool) {
        let Some(feedback) = self.feedback.as_ref() else {
            return;
        };

        if feedback.correct
            || feedback.question != Question::Meaning
            || feedback.typed.trim().is_empty()
        {
            return;
        }
        let typed = feedback.typed.trim().to_string();

        if add_synonym {
            match self.add_current_meaning_synonym(&typed) {
                Ok(()) => self.status = Some("Synonym added and answer accepted.".to_string()),
                Err(err) => {
                    self.status = Some(format!("Could not add synonym: {err}"));
                    return;
                }
            }
        } else {
            self.status = Some("Answer accepted for this session.".to_string());
        }

        self.correct_answers += 1;
        if self.screen == Screen::ReviewQuiz && self.incorrect_meaning_answers > 0 {
            self.incorrect_meaning_answers -= 1;
        }

        let action = self.accepted_meaning_action();
        if let Some(feedback) = self.feedback.as_mut() {
            feedback.correct = true;
            feedback.typo_accepted = false;
            feedback.action = action;
        }

        match self.screen {
            Screen::LessonQuiz => self.resolve_lesson_feedback(),
            Screen::ReviewQuiz => self.resolve_review_feedback(),
            _ => {}
        }
    }

    fn accepted_meaning_action(&self) -> FeedbackAction {
        let has_reading_question = self
            .current_subject()
            .map(ApiResource::has_reading_question)
            .unwrap_or(false);

        match self.screen {
            Screen::LessonQuiz => {
                if has_reading_question {
                    FeedbackAction::NextLessonQuestion
                } else {
                    FeedbackAction::NextLesson
                }
            }
            Screen::ReviewQuiz => {
                if has_reading_question {
                    FeedbackAction::NextReviewQuestion
                } else {
                    FeedbackAction::SubmitReview
                }
            }
            _ => FeedbackAction::ContinueQuestion,
        }
    }

    fn add_current_meaning_synonym(&self, synonym: &str) -> AppResult<()> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| "not signed in".to_string())?;
        let subject = self
            .current_subject()
            .ok_or_else(|| "no current subject".to_string())?;

        client.add_meaning_synonym(subject.id, synonym)
    }

    fn start_current_assignment(&mut self) {
        let Some(client) = self.client.as_ref() else {
            return;
        };
        let Some(subject) = self.current_subject() else {
            return;
        };
        let Some(assignment_id) = self.assignments_by_subject.get(&subject.id).copied() else {
            self.status = Some("No assignment found for this lesson.".to_string());
            return;
        };

        self.status = match client.start_assignment(assignment_id) {
            Ok(_) => Some("Lesson started in WaniKani.".to_string()),
            Err(err) => Some(format!("Could not start lesson: {err}")),
        };
        let write_status = self.status.clone();
        self.refresh_summary();
        if self.status.is_none() {
            self.status = write_status;
        }
    }

    fn submit_current_review(&mut self) {
        let Some(client) = self.client.as_ref() else {
            return;
        };
        let Some(subject) = self.current_subject() else {
            return;
        };
        let Some(assignment_id) = self.assignments_by_subject.get(&subject.id).copied() else {
            self.status = Some("No assignment found for this review.".to_string());
            return;
        };

        self.status = match client.create_review(
            assignment_id,
            self.incorrect_meaning_answers,
            self.incorrect_reading_answers,
        ) {
            Ok(_) => Some("Review submitted.".to_string()),
            Err(err) => Some(format!("Could not submit review: {err}")),
        };
        let write_status = self.status.clone();
        self.refresh_summary();
        if self.status.is_none() {
            self.status = write_status;
        }
    }

    fn current_subject(&self) -> Option<&ApiResource<SubjectData>> {
        self.subjects.get(self.subject_index)
    }

    fn available_count(&self, mode: SessionMode) -> usize {
        let Some(summary) = self.summary.as_ref() else {
            return 0;
        };
        match mode {
            SessionMode::Lessons => {
                unique_available_subject_count(&summary.data.lessons, Utc::now())
            }
            SessionMode::Reviews => {
                unique_available_subject_count(&summary.data.reviews, Utc::now())
            }
        }
    }

    fn refresh_summary(&mut self) {
        let Some(client) = self.client.as_ref() else {
            return;
        };

        match client.summary() {
            Ok(summary) => self.summary = Some(summary),
            Err(err) => self.status = Some(format!("Could not refresh summary: {err}")),
        }
    }

    fn render_login(&self, frame: &mut Frame) {
        let area = centered_rect(72, 17, frame.area());
        frame.render_widget(Clear, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(4),
                Constraint::Length(4),
                Constraint::Length(6),
            ])
            .split(area);

        let title = Paragraph::new("WaniKani Login")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(title, chunks[0]);

        let help = Paragraph::new(
            "Paste an API v2 token with permissions for user info, subjects, assignments, study materials, starting assignments, and creating reviews. A successful login is saved until you log out.",
        )
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::LEFT | Borders::RIGHT));
        frame.render_widget(help, chunks[1]);

        let masked = "*".repeat(self.token_input.len());
        let input = Paragraph::new(vec![
            Line::from(masked),
            Line::styled(
                format!("{} characters", self.token_input.trim().len()),
                Style::default().fg(Color::Gray),
            ),
        ])
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().title("API token").borders(Borders::ALL));
        frame.render_widget(input, chunks[2]);

        self.render_status_panel(
            frame,
            chunks[3],
            "Enter signs in. Ctrl+U or Delete clears the field. Esc quits.",
        );
    }

    fn render_menu(&self, frame: &mut Frame) {
        let outer = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(5),
                Constraint::Length(8),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(outer);

        let header = Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    "WaniKani TUI",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(&self.username, Style::default().fg(Color::Cyan)),
            ]),
            Line::from(""),
            Line::from("Use the keyboard to choose a study session."),
        ])
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(header, chunks[0]);

        let cards = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[1]);

        let lessons = stat_card(
            "Lessons",
            self.available_count(SessionMode::Lessons),
            "Press l",
            Color::Green,
        );
        frame.render_widget(lessons, cards[0]);

        let reviews = stat_card(
            "Reviews",
            self.available_count(SessionMode::Reviews),
            "Press r",
            Color::Magenta,
        );
        frame.render_widget(reviews, cards[1]);

        let menu = List::new([
            ListItem::new("r  Start reviews"),
            ListItem::new("l  Start lessons"),
            ListItem::new("t  Log out and remove saved token"),
            ListItem::new("q  Quit"),
        ])
        .block(Block::default().title("Actions").borders(Borders::ALL));
        frame.render_widget(menu, chunks[2]);

        self.render_status_panel(frame, chunks[3], "Ready.");
    }

    fn render_limit(&self, frame: &mut Frame, mode: SessionMode) {
        let area = centered_rect(60, 10, frame.area());
        frame.render_widget(Clear, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(1),
            ])
            .split(area);

        let title = Paragraph::new(format!("Start {}", mode.label_title()))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(title, chunks[0]);

        let available = Paragraph::new(format!("Available: {}", self.available_count(mode)))
            .block(Block::default().borders(Borders::LEFT | Borders::RIGHT));
        frame.render_widget(available, chunks[1]);

        let input = Paragraph::new(self.limit_input.as_str())
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().title("Session size").borders(Borders::ALL));
        frame.render_widget(input, chunks[2]);

        self.render_status_panel(frame, chunks[3], "Enter starts. Esc returns to menu.");
    }

    fn render_lesson_study(&self, frame: &mut Frame) {
        let Some(subject) = self.subjects.get(self.lesson_study_index) else {
            return;
        };

        let outer = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(6),
                Constraint::Min(12),
                Constraint::Length(3),
            ])
            .split(outer);

        let header = self.subject_header_at(subject, SessionMode::Lessons, self.lesson_study_index);
        frame.render_widget(header, chunks[0]);

        let mut lines = vec![
            Line::from(vec![
                Span::styled("Meanings: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(subject.accepted_meanings().join(", ")),
            ]),
            Line::from(vec![
                Span::styled("Readings: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(optional_join(subject.accepted_readings())),
            ]),
            Line::from(""),
        ];

        if let Some(mnemonic) = subject.data.meaning_mnemonic.as_deref() {
            lines.push(Line::styled(
                "Meaning mnemonic",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::from(strip_html(mnemonic)));
            lines.push(Line::from(""));
        }
        if let Some(mnemonic) = subject.data.reading_mnemonic.as_deref() {
            lines.push(Line::styled(
                "Reading mnemonic",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::from(strip_html(mnemonic)));
        }

        let body = Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().title("Lesson").borders(Borders::ALL));
        frame.render_widget(body, chunks[1]);

        self.render_status_panel(
            frame,
            chunks[2],
            "Left/p previous. Right/n next. Enter starts the batch quiz. Esc returns to menu.",
        );
    }

    fn render_quiz(&self, frame: &mut Frame, mode: SessionMode) {
        let Some(subject) = self.current_subject() else {
            return;
        };

        let outer = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(6),
                Constraint::Length(6),
                Constraint::Min(6),
                Constraint::Length(3),
            ])
            .split(outer);

        frame.render_widget(
            self.subject_header_at(subject, mode, self.subject_index),
            chunks[0],
        );

        let question_title = match self.question {
            Question::Meaning => "Meaning",
            Question::Reading => "Reading",
        };
        let answer = match self.question {
            Question::Meaning => self.answer_input.clone(),
            Question::Reading => self.answer_input.to_hiragana(),
        };
        let input = Paragraph::new(answer)
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().title(question_title).borders(Borders::ALL));
        frame.render_widget(input, chunks[1]);

        if let Some(feedback) = self.feedback.as_ref() {
            let title = if feedback.correct {
                if feedback.typo_accepted {
                    "Correct - typo accepted"
                } else {
                    "Correct"
                }
            } else {
                "Incorrect"
            };
            let style = if feedback.correct {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            let mut feedback_text = vec![
                Line::styled(title, style.add_modifier(Modifier::BOLD)),
                Line::from(""),
                Line::from(format!("You typed: {}", feedback.typed)),
                Line::from(format!("Correct answer: {}", feedback.expected.join(", "))),
                Line::from(""),
            ];
            if !feedback.correct && feedback.question == Question::Meaning {
                feedback_text.push(Line::from("a accepts your answer for this session."));
                feedback_text.push(Line::from("s adds it as a synonym and accepts it."));
            }
            feedback_text.push(Line::from("Press Enter to continue."));
            let panel = Paragraph::new(feedback_text)
                .wrap(Wrap { trim: true })
                .block(Block::default().title("Feedback").borders(Borders::ALL));
            frame.render_widget(panel, chunks[2]);
        } else {
            let help = match self.question {
                Question::Meaning => "Type the English meaning and press Enter.",
                Question::Reading => "Type romaji; it appears as kana here. Press Enter to submit.",
            };
            let panel = Paragraph::new(help)
                .wrap(Wrap { trim: true })
                .block(Block::default().title("Prompt").borders(Borders::ALL));
            frame.render_widget(panel, chunks[2]);
        }

        self.render_status_panel(frame, chunks[3], "Esc returns to menu. Ctrl+C quits.");
    }

    fn render_done(&self, frame: &mut Frame, mode: SessionMode) {
        let area = centered_rect(70, 12, frame.area());
        frame.render_widget(Clear, area);
        let title = format!("{} complete", mode.label_title());
        let mut lines = vec![
            Line::styled(title, Style::default().add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::from(format!(
                "Answers: {}/{} correct",
                self.correct_answers, self.total_answers
            )),
        ];
        if let Some(status) = self.status.as_ref() {
            lines.push(Line::from(""));
            lines.push(Line::from(status.as_str()));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Enter returns to menu. q quits."));

        let panel = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(panel, area);
    }

    fn subject_header_at(
        &self,
        subject: &ApiResource<SubjectData>,
        mode: SessionMode,
        index: usize,
    ) -> Paragraph<'static> {
        let progress = format!(
            "{} {}/{}",
            mode.singular_title(),
            index + 1,
            self.subjects.len()
        );
        let kind = subject.kind_label().to_string();
        let display_name = subject.display_name();
        let lines = vec![
            Line::from(vec![
                Span::styled(progress, Style::default().fg(Color::Cyan)),
                Span::raw("  "),
                Span::styled(kind, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(""),
            Line::styled(display_name, Style::default().add_modifier(Modifier::BOLD)),
        ];
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL))
    }

    fn render_status_panel(&self, frame: &mut Frame, area: Rect, fallback: &str) {
        let message = self
            .error
            .as_deref()
            .or(self.status.as_deref())
            .unwrap_or(fallback);
        let color = if self.error.is_some() {
            Color::Red
        } else {
            Color::Gray
        };
        let status = Paragraph::new(message)
            .style(Style::default().fg(color))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(status, area);
    }
}

impl SessionMode {
    fn label_lower(self) -> &'static str {
        match self {
            SessionMode::Lessons => "lessons",
            SessionMode::Reviews => "reviews",
        }
    }

    fn label_title(self) -> &'static str {
        match self {
            SessionMode::Lessons => "Lessons",
            SessionMode::Reviews => "Reviews",
        }
    }

    fn singular_title(self) -> &'static str {
        match self {
            SessionMode::Lessons => "Lesson",
            SessionMode::Reviews => "Review",
        }
    }
}

#[derive(Clone)]
struct WaniKaniClient {
    token: String,
}

impl WaniKaniClient {
    fn new(token: String) -> Self {
        Self { token }
    }

    fn user(&self) -> AppResult<UserResponse> {
        self.get_json(&format!("{API_BASE}/user"))
    }

    fn summary(&self) -> AppResult<SummaryResponse> {
        self.get_json(&format!("{API_BASE}/summary"))
    }

    fn subjects(&self, subject_ids: &[u64]) -> AppResult<Vec<ApiResource<SubjectData>>> {
        let mut subjects = Vec::new();
        for chunk in subject_ids.chunks(100) {
            let ids = join_ids(chunk);
            let url = format!("{API_BASE}/subjects?ids={ids}");
            let page: CollectionResponse<SubjectData> = self.get_json(&url)?;
            subjects.extend(page.data);
        }

        let requested_order: HashMap<u64, usize> = subject_ids
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index))
            .collect();
        subjects.sort_by_key(|subject| {
            requested_order
                .get(&subject.id)
                .copied()
                .unwrap_or(usize::MAX)
        });
        Ok(subjects)
    }

    fn assignments_for_subjects(
        &self,
        subject_ids: &[u64],
    ) -> AppResult<Vec<ApiResource<AssignmentData>>> {
        let mut assignments = Vec::new();
        for chunk in subject_ids.chunks(100) {
            let ids = join_ids(chunk);
            let url = format!("{API_BASE}/assignments?subject_ids={ids}");
            let page: CollectionResponse<AssignmentData> = self.get_json(&url)?;
            assignments.extend(page.data);
        }
        Ok(assignments)
    }

    fn start_assignment(&self, assignment_id: u64) -> AppResult<()> {
        let url = format!("{API_BASE}/assignments/{assignment_id}/start");
        let body = serde_json::json!({ "assignment": {} });
        self.put_json(&url, body)?;
        Ok(())
    }

    fn create_review(
        &self,
        assignment_id: u64,
        incorrect_meaning_answers: u32,
        incorrect_reading_answers: u32,
    ) -> AppResult<()> {
        let body = serde_json::json!({
            "review": {
                "assignment_id": assignment_id,
                "incorrect_meaning_answers": incorrect_meaning_answers,
                "incorrect_reading_answers": incorrect_reading_answers
            }
        });
        self.post_json(&format!("{API_BASE}/reviews"), body)?;
        Ok(())
    }

    fn add_meaning_synonym(&self, subject_id: u64, synonym: &str) -> AppResult<()> {
        let synonym = synonym.trim();
        if synonym.is_empty() {
            return Err("synonym cannot be empty".into());
        }

        let existing = self.study_material_for_subject(subject_id)?;
        match existing {
            Some(material) => {
                let mut synonyms = material.data.meaning_synonyms;
                if !synonyms
                    .iter()
                    .any(|existing| normalize_answer(existing) == normalize_answer(synonym))
                {
                    synonyms.push(synonym.to_string());
                }

                let body = serde_json::json!({
                    "study_material": {
                        "meaning_synonyms": synonyms
                    }
                });
                self.put_json(&format!("{API_BASE}/study_materials/{}", material.id), body)?;
            }
            None => {
                let body = serde_json::json!({
                    "study_material": {
                        "subject_id": subject_id,
                        "meaning_synonyms": [synonym]
                    }
                });
                self.post_json(&format!("{API_BASE}/study_materials"), body)?;
            }
        }

        Ok(())
    }

    fn study_material_for_subject(
        &self,
        subject_id: u64,
    ) -> AppResult<Option<ApiResource<StudyMaterialData>>> {
        let url = format!("{API_BASE}/study_materials?subject_ids={subject_id}");
        let page: CollectionResponse<StudyMaterialData> = self.get_json(&url)?;
        Ok(page.data.into_iter().next())
    }

    fn get_json<T>(&self, url: &str) -> AppResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let response = ureq::get(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Wanikani-Revision", "20170710")
            .call()
            .map_err(ApiError::from)?;
        Ok(response.into_json()?)
    }

    fn put_json(&self, url: &str, body: serde_json::Value) -> AppResult<serde_json::Value> {
        let response = ureq::put(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Wanikani-Revision", "20170710")
            .send_json(body)
            .map_err(ApiError::from)?;
        Ok(response.into_json()?)
    }

    fn post_json(&self, url: &str, body: serde_json::Value) -> AppResult<serde_json::Value> {
        let response = ureq::post(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Wanikani-Revision", "20170710")
            .send_json(body)
            .map_err(ApiError::from)?;
        Ok(response.into_json()?)
    }
}

#[derive(Debug)]
struct ApiError(String);

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ApiError {}

impl From<ureq::Error> for ApiError {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::Status(code, response) => {
                let message = response
                    .into_string()
                    .unwrap_or_else(|_| "no response body".to_string());
                Self(format!("WaniKani returned HTTP {code}: {message}"))
            }
            ureq::Error::Transport(transport) => Self(format!("network error: {transport}")),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiResource<T> {
    id: u64,
    object: String,
    data: T,
}

#[derive(Debug, Deserialize)]
struct CollectionResponse<T> {
    data: Vec<ApiResource<T>>,
}

#[derive(Debug, Deserialize)]
struct UserResponse {
    data: UserData,
}

#[derive(Debug, Deserialize)]
struct UserData {
    username: String,
}

#[derive(Debug, Deserialize)]
struct SummaryResponse {
    data: SummaryData,
}

#[derive(Debug, Deserialize)]
struct SummaryData {
    #[serde(default)]
    lessons: Vec<SummaryBucket>,
    #[serde(default)]
    reviews: Vec<SummaryBucket>,
}

#[derive(Debug, Deserialize)]
struct SummaryBucket {
    #[serde(default)]
    available_at: Option<String>,
    #[serde(default)]
    subject_ids: Vec<u64>,
}

#[derive(Debug, Deserialize)]
struct SubjectData {
    #[serde(default)]
    characters: Option<String>,
    slug: String,
    #[serde(default)]
    meanings: Vec<Meaning>,
    #[serde(default)]
    readings: Vec<Reading>,
    #[serde(default)]
    meaning_mnemonic: Option<String>,
    #[serde(default)]
    reading_mnemonic: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Meaning {
    meaning: String,
    #[serde(default)]
    accepted_answer: bool,
}

#[derive(Debug, Deserialize)]
struct Reading {
    reading: String,
    #[serde(default)]
    accepted_answer: bool,
}

#[derive(Debug, Deserialize)]
struct AssignmentData {
    subject_id: u64,
}

#[derive(Debug, Deserialize)]
struct StudyMaterialData {
    #[serde(default)]
    meaning_synonyms: Vec<String>,
}

impl ApiResource<SubjectData> {
    fn display_name(&self) -> String {
        self.data
            .characters
            .as_ref()
            .filter(|characters| !characters.is_empty())
            .cloned()
            .unwrap_or_else(|| self.data.slug.clone())
    }

    fn kind_label(&self) -> &str {
        match self.object.as_str() {
            "radical" => "Radical",
            "kanji" => "Kanji",
            "vocabulary" => "Vocabulary",
            "kana_vocabulary" => "Kana vocabulary",
            _ => "Subject",
        }
    }

    fn accepted_meanings(&self) -> Vec<String> {
        let accepted: Vec<String> = self
            .data
            .meanings
            .iter()
            .filter(|meaning| meaning.accepted_answer)
            .map(|meaning| meaning.meaning.clone())
            .collect();

        if accepted.is_empty() {
            self.data
                .meanings
                .iter()
                .map(|meaning| meaning.meaning.clone())
                .collect()
        } else {
            accepted
        }
    }

    fn accepted_readings(&self) -> Vec<String> {
        let accepted: Vec<String> = self
            .data
            .readings
            .iter()
            .filter(|reading| reading.accepted_answer)
            .map(|reading| reading.reading.clone())
            .collect();

        if accepted.is_empty() {
            self.data
                .readings
                .iter()
                .map(|reading| reading.reading.clone())
                .collect()
        } else {
            accepted
        }
    }

    fn has_reading_question(&self) -> bool {
        matches!(self.object.as_str(), "kanji" | "vocabulary")
            && !self.accepted_readings().is_empty()
    }
}

impl SummaryBucket {
    fn is_available_at(&self, now: DateTime<Utc>) -> bool {
        let Some(available_at) = self.available_at.as_deref() else {
            return true;
        };

        DateTime::parse_from_rfc3339(available_at)
            .map(|available_at| available_at.with_timezone(&Utc) <= now)
            .unwrap_or(false)
    }
}

fn matches_answer(answer: &str, accepted_answers: &[String]) -> bool {
    let normalized = normalize_answer(answer);
    accepted_answers
        .iter()
        .any(|candidate| normalize_answer(candidate) == normalized)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnswerGrade {
    Correct,
    Typo,
    Incorrect,
}

impl AnswerGrade {
    fn is_accepted(self) -> bool {
        matches!(self, AnswerGrade::Correct | AnswerGrade::Typo)
    }
}

fn grade_meaning_answer(answer: &str, accepted_answers: &[String]) -> AnswerGrade {
    let normalized = normalize_answer(answer);
    if normalized.is_empty() {
        return AnswerGrade::Incorrect;
    }

    let mut typo_match = false;
    for candidate in accepted_answers {
        let candidate = normalize_answer(candidate);
        if candidate == normalized {
            return AnswerGrade::Correct;
        }
        if is_meaning_typo(&normalized, &candidate) {
            typo_match = true;
        }
    }

    if typo_match {
        AnswerGrade::Typo
    } else {
        AnswerGrade::Incorrect
    }
}

fn is_meaning_typo(answer: &str, candidate: &str) -> bool {
    let min_len = answer.chars().count().min(candidate.chars().count());
    if min_len < 4 {
        return false;
    }

    let distance = damerau_levenshtein(answer, candidate);
    let threshold = if min_len >= 8 { 2 } else { 1 };
    distance <= threshold
}

fn damerau_levenshtein(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut distances = vec![vec![0; right.len() + 1]; left.len() + 1];

    for (index, row) in distances.iter_mut().enumerate() {
        row[0] = index;
    }
    for index in 0..=right.len() {
        distances[0][index] = index;
    }

    for i in 1..=left.len() {
        for j in 1..=right.len() {
            let substitution_cost = usize::from(left[i - 1] != right[j - 1]);
            let mut distance = (distances[i - 1][j] + 1)
                .min(distances[i][j - 1] + 1)
                .min(distances[i - 1][j - 1] + substitution_cost);

            if i > 1 && j > 1 && left[i - 1] == right[j - 2] && left[i - 2] == right[j - 1] {
                distance = distance.min(distances[i - 2][j - 2] + 1);
            }

            distances[i][j] = distance;
        }
    }

    distances[left.len()][right.len()]
}

fn normalize_answer(answer: &str) -> String {
    answer
        .trim()
        .to_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '-' && *character != '_')
        .collect()
}

fn collect_available_subject_ids(
    buckets: &[SummaryBucket],
    limit: usize,
    now: DateTime<Utc>,
) -> Vec<u64> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();

    for bucket in buckets.iter().filter(|bucket| bucket.is_available_at(now)) {
        for id in &bucket.subject_ids {
            if seen.insert(*id) {
                ids.push(*id);
                if ids.len() == limit {
                    return ids;
                }
            }
        }
    }

    ids
}

fn unique_available_subject_count(buckets: &[SummaryBucket], now: DateTime<Utc>) -> usize {
    buckets
        .iter()
        .filter(|bucket| bucket.is_available_at(now))
        .flat_map(|bucket| bucket.subject_ids.iter())
        .collect::<HashSet<_>>()
        .len()
}

fn assignment_map(assignments: Vec<ApiResource<AssignmentData>>) -> HashMap<u64, u64> {
    assignments
        .into_iter()
        .map(|assignment| (assignment.data.subject_id, assignment.id))
        .collect()
}

fn join_ids(ids: &[u64]) -> String {
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn optional_join(values: Vec<String>) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

fn load_stored_token() -> Option<String> {
    let path = token_storage_path().ok()?;
    let token = fs::read_to_string(path).ok()?;
    let token = token.trim().to_string();
    (!token.is_empty()).then_some(token)
}

fn save_stored_token(token: &str) -> io::Result<()> {
    let path = token_storage_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, token.trim())
}

fn delete_stored_token() -> io::Result<()> {
    let path = token_storage_path()?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn token_storage_path() -> io::Result<PathBuf> {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return Ok(PathBuf::from(appdata).join("wanikani-tui").join("token"));
    }

    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return Ok(PathBuf::from(home).join(".wanikani-tui-token"));
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "could not find APPDATA or HOME for token storage",
    ))
}

fn stat_card<'a>(title: &'a str, count: usize, hint: &'a str, color: Color) -> Paragraph<'a> {
    Paragraph::new(vec![
        Line::styled(title, Style::default().add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::styled(
            count.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Line::from(hint),
    ])
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL))
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn strip_html(input: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;

    for character in input.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }

    output
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meaning_matching_ignores_case_spaces_and_hyphens() {
        let accepted = vec!["All Things".to_string()];
        assert!(matches_answer("all-things", &accepted));
        assert!(matches_answer(" ALL THINGS ", &accepted));
    }

    #[test]
    fn reading_matching_accepts_romaji_after_hiragana_conversion() {
        let accepted = vec!["\u{304c}\u{3063}\u{3053}\u{3046}".to_string()];
        let typed = "gakkou".to_hiragana();
        assert!(matches_answer(&typed, &accepted));
    }

    #[test]
    fn meaning_grading_accepts_small_typos() {
        let accepted = vec!["angle".to_string(), "corner".to_string()];
        assert_eq!(grade_meaning_answer("angel", &accepted), AnswerGrade::Typo);
        assert_eq!(grade_meaning_answer("anglx", &accepted), AnswerGrade::Typo);
        assert_eq!(
            grade_meaning_answer("completely wrong", &accepted),
            AnswerGrade::Incorrect
        );
    }

    #[test]
    fn available_count_ignores_future_summary_buckets() {
        let now = DateTime::parse_from_rfc3339("2026-06-13T12:00:00.000000Z")
            .unwrap()
            .with_timezone(&Utc);
        let buckets = vec![
            SummaryBucket {
                available_at: Some("2026-06-13T11:00:00.000000Z".to_string()),
                subject_ids: vec![1, 2, 3],
            },
            SummaryBucket {
                available_at: Some("2026-06-13T13:00:00.000000Z".to_string()),
                subject_ids: vec![4, 5],
            },
        ];

        assert_eq!(unique_available_subject_count(&buckets, now), 3);
        assert_eq!(
            collect_available_subject_ids(&buckets, 10, now),
            vec![1, 2, 3]
        );
    }
}
