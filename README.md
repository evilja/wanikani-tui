# wanikani-tui

A terminal WaniKani client for lessons and reviews.

## Run

```powershell
cargo run
```

The app asks for a WaniKani API v2 token on startup. The token needs permissions to:

- read user information
- read subjects
- read assignments
- read and write study materials
- start assignments
- create reviews

After a successful login, the token is saved locally so you do not need to paste it every time. On Windows it is stored at `%APPDATA%\wanikani-tui\token`. Use `t` from the main menu to log out and remove the saved token.

## Features

- Login screen that prompts for a WaniKani API token
- App-style TUI with framed login, menu, lesson, review, and feedback screens
- Main menu showing available lesson and review counts
- Lesson sessions with subject meanings, readings, and mnemonics
- Review sessions that submit results to WaniKani
- Romaji-to-kana conversion in reading prompts while typing
- Correct answers shown immediately after wrong answers

## Keys

- `r`: start reviews from the main menu
- `l`: start lessons from the main menu
- `t`: log out and remove the saved token
- `Right` or `n`: next item on the lesson study screen
- `Left` or `p`: previous item on the lesson study screen
- `Enter`: submit the current screen/input; on the lesson study screen, start the whole batch quiz
- `a`: accept an incorrect meaning answer for the current session
- `s`: add an incorrect meaning answer as a synonym and accept it
- `Ctrl+U` or `Delete`: clear the token field on the login screen
- `Esc`: return to the menu or quit from login
- `Ctrl+C`: quit
