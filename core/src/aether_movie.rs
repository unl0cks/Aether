//! Which movie is AQW.
//!
//! Every Aether repair is gated on recognising the movie it belongs to, and until 0.5.14 each gate
//! spelled that out for itself as "the URL contains spider.swf". Four modules, four copies, one
//! assumption: that the game has a fixed file name.
//!
//! It does not. `Loader3.swf`, the loader the live client embeds, asks `api/data/gameversion` which
//! build is current and loads whatever it is told, so the game movie is `Game3098r24.swf` at the
//! time of writing and something else after the next release. `spider.swf` is the fixed name
//! Artix's staging loader hardcodes, and it is a frozen older build; Aether was pointed at the
//! staging loader by mistake, which is why it reported itself as game version 4.26 while everyone
//! else was on 4.361.
//!
//! So the question is asked once, here. A gate that drifts from its neighbours does not fail
//! loudly. It silently stops repairing something, and a client ends up quietly missing half of
//! what it was built to do.

/// The file a URL or path names, with any query string or fragment removed.
///
/// Backslashes count as separators so a Windows path to a local copy resolves the same way a URL
/// does.
pub fn url_file_name(url: &str) -> &str {
    let path = url
        .split_once(['?', '#'])
        .map_or(url, |(path, _)| path);
    path.rsplit_once(['/', '\\'])
        .map_or(path, |(_, file_name)| file_name)
}

/// Whether a movie URL names AQW's loader, either the live one or the staging one.
///
/// The loader is not a wrapper that goes away once the game arrives. It stays on the stage as the
/// game's ancestor and keeps the five top-level frame labels the game asks it to play, so a repair
/// about those labels is a repair about this movie rather than about the game.
pub fn is_aqw_loader_movie(movie_url: &str) -> bool {
    is_aqw_loader_file(url_file_name(movie_url))
}

fn is_aqw_loader_file(file_name: &str) -> bool {
    file_name.eq_ignore_ascii_case("Loader3.swf")
        || file_name.eq_ignore_ascii_case("Loader_Spider.swf")
}

/// Whether a movie URL names the AQW game movie, whichever build the loader chose.
pub fn is_aqw_game_movie(movie_url: &str) -> bool {
    let file_name = url_file_name(movie_url);

    // Asked first, because the staging loader is called `Loader_Spider.swf` and every other rule
    // here would otherwise mistake its last ten characters for the game it loads.
    if is_aqw_loader_file(file_name) {
        return false;
    }

    file_name.eq_ignore_ascii_case("spider.swf")
        || ends_with_ignore_ascii_case(file_name, "_spider.swf")
        || names_versioned_game_build(movie_url, file_name)
}

/// The same, and served from AQW itself rather than merely named like it.
pub fn is_hosted_aqw_game_movie(movie_url: &str) -> bool {
    contains_ignore_ascii_case(movie_url, "game.aq.com/game/gamefiles/") && is_aqw_game_movie(movie_url)
}

/// Whether a URL names a versioned game build, as `.../gamefiles/Game3098r24.swf`.
///
/// The build number changes every release, so only the shape is matched: a name that starts `Game`
/// and ends `.swf`, sitting directly in `gamefiles/`. Depth is what rejects the near miss --
/// `gamefiles/maps/tradeskills/spellcraft/game-spellcraftr2.swf` begins with the same four letters
/// and is a map, not the game.
fn names_versioned_game_build(movie_url: &str, file_name: &str) -> bool {
    const DIRECTORY: &str = "gamefiles/";

    let name = file_name.as_bytes();
    if name.len() <= "Game.swf".len()
        || !name[..4].eq_ignore_ascii_case(b"Game")
        || !name[name.len() - 4..].eq_ignore_ascii_case(b".swf")
    {
        return false;
    }

    let Some(start) = find_ignore_ascii_case(movie_url, DIRECTORY) else {
        return false;
    };
    let below = &movie_url[start + DIRECTORY.len()..];
    let below = &below[..below.find(['?', '#']).unwrap_or(below.len())];
    !below.contains(['/', '\\'])
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    find_ignore_ascii_case(haystack, needle).is_some()
}

fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn ends_with_ignore_ascii_case(text: &str, suffix: &str) -> bool {
    let text = text.as_bytes();
    let suffix = suffix.as_bytes();
    text.len() >= suffix.len() && text[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `Loader3.swf` loads today, and what it will load once the build number moves.
    #[test]
    fn a_versioned_build_is_the_game() {
        assert!(is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Game3098r24.swf?ver=R0047"
        ));
        assert!(is_hosted_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Game3098r24.swf?ver=R0047"
        ));
        assert!(is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Game4100r1.swf"
        ));
        assert!(is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/GAME3098R24.SWF"
        ));
    }

    /// The frozen staging build Aether ran until 0.5.14. Anyone still pointed at it keeps every
    /// repair, because all of them were written against it.
    #[test]
    fn the_staging_build_is_still_the_game() {
        assert!(is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=0.6"
        ));
        assert!(is_hosted_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/spider.swf?ver=0.6"
        ));
        assert!(is_aqw_game_movie("file:///E:/captures/test_spider.swf"));
        assert!(is_aqw_game_movie("E:\\captures\\test_spider.swf"));
    }

    /// The loader is the game's ancestor, not the game. Its name ends in the ten characters that
    /// used to be the whole test, which is what made the old gates accept it.
    #[test]
    fn the_loader_is_never_the_game() {
        assert!(is_aqw_loader_movie(
            "https://game.aq.com/game/gamefiles/Loader3.swf?ver=a"
        ));
        assert!(is_aqw_loader_movie(
            "https://game.aq.com/game/gamefiles/Loader_Spider.swf"
        ));
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Loader3.swf?ver=a"
        ));
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/Loader_Spider.swf"
        ));
    }

    #[test]
    fn maps_assets_and_other_hosts_are_not_the_game() {
        // Begins with the same four letters, and is a map.
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/maps/tradeskills/spellcraft/game-spellcraftr2.swf"
        ));
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/assets/assets_2026.swf"
        ));
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/notspider.swf"
        ));
        assert!(!is_aqw_game_movie(
            "https://game.aq.com/game/gamefiles/spider.swf.txt"
        ));
        // Named like the game, served by somebody else. The unhosted test still accepts it, which
        // is what lets a local capture be replayed with the repairs on.
        assert!(!is_hosted_aqw_game_movie(
            "https://example.invalid/gamefiles/Game3098r24.swf"
        ));
        assert!(is_aqw_game_movie(
            "https://example.invalid/gamefiles/Game3098r24.swf"
        ));
        // The shape without a name, and a name without the rest of the file.
        assert!(!is_aqw_game_movie("https://game.aq.com/game/gamefiles/.swf"));
        assert!(!is_aqw_game_movie("https://game.aq.com/game/gamefiles/Game"));
        assert!(!is_aqw_game_movie(""));
    }
}
