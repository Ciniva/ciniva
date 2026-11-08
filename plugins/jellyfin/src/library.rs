use crate::{
    app::{json, not_found, ApiResult, Auth, Shared, Q},
    ciniva::{Cast, Episode, File, Movie, MovieCard, SeasonCard, Show, ShowCard},
    ids::{Id, Kind},
    items::{self, Ctx},
};
use axum::{
    extract::{Path, State},
    response::Response,
};
use futures_util::{stream, StreamExt, TryStreamExt};
use serde_json::{json, Value};
use std::cmp::Ordering;

pub enum Resolved {
    Movie(Movie),
    Series(Show),
    Season(Show, SeasonCard),
    Episode(Show, Episode),
    View(Kind),
    Root,
    Person(String, Cast),
}

pub fn ctx<'a>(st: &'a Shared, auth: &'a Auth) -> Ctx<'a> {
    Ctx { server_id: &st.server_id, token: &auth.token }
}

pub async fn resolve(st: &Shared, token: &str, raw: &str) -> ApiResult<Resolved> {
    let id = Id::decode(raw).ok_or_else(not_found)?;
    let c = &st.ciniva;
    Ok(match id.kind {
        Kind::Movie => Resolved::Movie(c.movie(token, id.id).await?),
        Kind::Series => Resolved::Series(c.show(token, id.parent).await?),
        Kind::Season => {
            let show = c.show(token, id.parent).await?;
            let card = show.seasons.iter().find(|s| s.season_number == id.season).cloned().ok_or_else(not_found)?;
            Resolved::Season(show, card)
        }
        Kind::Episode => {
            let (show, eps) = tokio::try_join!(c.show(token, id.parent), c.season(token, id.parent, id.season))?;
            let ep = eps.into_iter().find(|e| e.id == id.id).ok_or_else(not_found)?;
            Resolved::Episode(show, ep)
        }
        Kind::MoviesView | Kind::ShowsView => Resolved::View(id.kind),
        Kind::Root => Resolved::Root,
        Kind::MoviePerson => {
            let m = c.movie(token, id.parent).await?;
            Resolved::Person(raw.to_string(), m.cast.get(id.id as usize).cloned().ok_or_else(not_found)?)
        }
        Kind::ShowPerson => {
            let s = c.show(token, id.parent).await?;
            Resolved::Person(raw.to_string(), s.cast.get(id.id as usize).cloned().ok_or_else(not_found)?)
        }
        Kind::User | Kind::File => return Err(not_found()),
    })
}

pub async fn item_value(st: &Shared, auth: &Auth, r: &Resolved) -> ApiResult<Value> {
    let cx = ctx(st, auth);
    Ok(match r {
        Resolved::Movie(m) => items::movie(&cx, m),
        Resolved::Series(s) => items::show(&cx, s, None),
        Resolved::Season(s, c) => items::season(&cx, s, c),
        Resolved::Episode(s, e) => items::episode(&cx, s, e),
        Resolved::View(k) => {
            let count = if *k == Kind::MoviesView {
                st.ciniva.movies(&auth.token, "title").await?.len()
            } else {
                st.ciniva.shows(&auth.token, "title").await?.len()
            };
            items::view(&cx, *k, Some(count))
        }
        Resolved::Root => items::root(&cx),
        Resolved::Person(id, c) => items::person(&cx, id, c),
    })
}

/// The files that play for an item id or a media source id.
pub async fn files_for(st: &Shared, token: &str, raw: &str) -> ApiResult<(String, Vec<File>, String)> {
    if let Some(Id { kind: Kind::File, id, .. }) = Id::decode(raw) {
        let f = st.ciniva.file(token, id).await?;
        return Ok((raw.to_string(), vec![f], String::new()));
    }
    match resolve(st, token, raw).await? {
        Resolved::Movie(m) => Ok((Id::movie(m.id), m.files, m.title)),
        Resolved::Episode(s, e) => {
            let name = e.title.clone().unwrap_or_else(|| s.title.clone());
            Ok((Id::episode(s.id, e.season_number, e.id), e.files, name))
        }
        _ => Err(not_found()),
    }
}

pub async fn season_episodes(st: &Shared, token: &str, show: &Show, seasons: &[i64]) -> ApiResult<Vec<Episode>> {
    let lists: Vec<Vec<Episode>> = stream::iter(seasons.iter().copied())
        .map(|n| async move { st.ciniva.season(token, show.id, n).await })
        .buffered(6)
        .try_collect()
        .await?;
    Ok(lists.into_iter().flatten().filter(|e| !e.files.is_empty()).collect())
}

fn season_order(show: &Show) -> Vec<i64> {
    let mut n: Vec<i64> = show.seasons.iter().map(|s| s.season_number).collect();
    n.sort_by_key(|s| (*s == 0, *s));
    n
}

async fn all_shows_full(st: &Shared, token: &str) -> ApiResult<Vec<Show>> {
    let cards = st.ciniva.shows(token, "title").await?;
    Ok(stream::iter(cards).map(|c| async move { st.ciniva.show(token, c.id).await }).buffered(6).try_collect().await?)
}

async fn all_episodes(st: &Shared, auth: &Auth) -> ApiResult<Vec<Value>> {
    let shows = all_shows_full(st, &auth.token).await?;
    let mut out = Vec::new();
    for s in &shows {
        let eps = season_episodes(st, &auth.token, s, &season_order(s)).await?;
        out.extend(eps.iter().map(|e| items::episode(&ctx(st, auth), s, e)));
    }
    Ok(out)
}

async fn all_seasons(st: &Shared, auth: &Auth) -> ApiResult<Vec<Value>> {
    let shows = all_shows_full(st, &auth.token).await?;
    Ok(shows.iter().flat_map(|s| s.seasons.iter().map(|c| items::season(&ctx(st, auth), s, c)).collect::<Vec<_>>()).collect())
}

fn movie_cards(st: &Shared, auth: &Auth, cards: &[MovieCard]) -> Vec<Value> {
    cards.iter().map(|c| items::movie_card(&ctx(st, auth), c)).collect()
}

fn show_cards(st: &Shared, auth: &Auth, cards: &[ShowCard]) -> Vec<Value> {
    cards.iter().map(|c| items::show_card(&ctx(st, auth), c)).collect()
}

/// Replaces list cards with full items (overview, genres, people, media sources) for the page actually returned.
pub async fn enrich(st: &Shared, auth: &Auth, page: Vec<Value>) -> Vec<Value> {
    if page.len() > 200 {
        return page;
    }
    stream::iter(page)
        .map(|card| async move {
            let id = card["Id"].as_str().and_then(Id::decode);
            let full = match (card["Type"].as_str(), id) {
                (Some("Movie"), Some(id)) if card.get("MediaSources").is_none() => {
                    st.ciniva.movie(&auth.token, id.id).await.ok().map(|m| items::movie(&ctx(st, auth), &m))
                }
                (Some("Series"), Some(id)) if card.get("Overview").is_none() => {
                    st.ciniva.show(&auth.token, id.parent).await.ok().map(|s| items::show(&ctx(st, auth), &s, None))
                }
                _ => None,
            };
            match full {
                Some(mut f) => {
                    if let (Some(fo), Some(co)) = (f.as_object_mut(), card.as_object()) {
                        for (k, v) in co {
                            fo.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                    f
                }
                None => card,
            }
        })
        .buffered(8)
        .collect()
        .await
}

fn str_key(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

fn cmp_by(a: &Value, b: &Value, field: &str) -> Ordering {
    match field {
        "sortname" | "name" | "seriessortname" => str_key(a, "SortName").cmp(&str_key(b, "SortName")),
        "datecreated" | "datelastcontentadded" | "datelastmediaadded" => str_key(a, "DateCreated").cmp(&str_key(b, "DateCreated")),
        "premieredate" | "productionyear" | "startdate" | "airtime" => {
            (a["ProductionYear"].as_i64(), str_key(a, "PremiereDate")).cmp(&(b["ProductionYear"].as_i64(), str_key(b, "PremiereDate")))
        }
        "communityrating" | "criticrating" => a["CommunityRating"].as_f64().unwrap_or(0.0).total_cmp(&b["CommunityRating"].as_f64().unwrap_or(0.0)),
        "runtime" => a["RunTimeTicks"].as_i64().cmp(&b["RunTimeTicks"].as_i64()),
        "parentindexnumber" | "indexnumber" | "airedepisodeorder" => {
            (a["ParentIndexNumber"].as_i64(), a["IndexNumber"].as_i64()).cmp(&(b["ParentIndexNumber"].as_i64(), b["IndexNumber"].as_i64()))
        }
        "playcount" => a["UserData"]["PlayCount"].as_i64().cmp(&b["UserData"]["PlayCount"].as_i64()),
        "dateplayed" => a["UserData"]["PlaybackPositionTicks"].as_i64().cmp(&b["UserData"]["PlaybackPositionTicks"].as_i64()),
        "isfolder" => a["IsFolder"].as_bool().cmp(&b["IsFolder"].as_bool()),
        _ => Ordering::Equal,
    }
}

pub fn sort(items: &mut [Value], q: &Q) {
    let fields: Vec<String> = q.list("sortby").iter().map(|s| s.to_ascii_lowercase()).collect();
    if fields.is_empty() {
        return;
    }
    if fields.iter().any(|f| f == "random") {
        use rand::seq::SliceRandom;
        items.shuffle(&mut rand::thread_rng());
        return;
    }
    let orders: Vec<bool> = q.list("sortorder").iter().map(|o| o.eq_ignore_ascii_case("descending")).collect();
    items.sort_by(|a, b| {
        for (i, f) in fields.iter().enumerate() {
            let desc = orders.get(i).or(orders.first()).copied().unwrap_or(false);
            let o = cmp_by(a, b, f);
            let o = if desc { o.reverse() } else { o };
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    });
}

fn played(v: &Value) -> bool {
    v["UserData"]["Played"].as_bool().unwrap_or(false)
}

fn resumable(v: &Value) -> bool {
    v["UserData"]["PlaybackPositionTicks"].as_i64().unwrap_or(0) > 0 && !played(v)
}

pub fn filter(items: Vec<Value>, q: &Q) -> Vec<Value> {
    let filters: Vec<String> = q.list("filters").iter().map(|s| s.to_ascii_lowercase()).collect();
    let include: Vec<String> = q.list("includeitemtypes").iter().map(|s| s.to_ascii_lowercase()).collect();
    let exclude: Vec<String> = q.list("excludeitemtypes").iter().map(|s| s.to_ascii_lowercase()).collect();
    let exclude_ids: Vec<String> = q.list("excludeitemids").iter().map(|s| s.replace('-', "").to_ascii_lowercase()).collect();
    let years: Vec<i64> = q.list("years").iter().filter_map(|y| y.parse().ok()).collect();
    let genres: Vec<String> = q.list("genres").iter().map(|s| s.to_ascii_lowercase()).collect();
    let term = q.get("searchterm").map(|t| t.to_lowercase());
    let starts = q.get("namestartswith").map(|t| t.to_lowercase());
    let starts_or_greater = q.get("namestartswithorgreater").map(|t| t.to_lowercase());
    let less_than = q.get("namelessthan").map(|t| t.to_lowercase());
    let is_played = q.bool("isplayed");
    items
        .into_iter()
        .filter(|v| {
            let ty = v["Type"].as_str().unwrap_or("").to_ascii_lowercase();
            let name = v["Name"].as_str().unwrap_or("").to_lowercase();
            let sort_name = v["SortName"].as_str().unwrap_or("").to_lowercase();
            (include.is_empty() || include.contains(&ty))
                && !exclude.contains(&ty)
                && !exclude_ids.contains(&v["Id"].as_str().unwrap_or("").to_string())
                && (!filters.iter().any(|f| f == "isplayed") || played(v))
                && (!filters.iter().any(|f| f == "isunplayed") || !played(v))
                && (!filters.iter().any(|f| f == "isresumable") || resumable(v))
                && (!filters.iter().any(|f| f == "isfolder") || v["IsFolder"].as_bool() == Some(true))
                && (!filters.iter().any(|f| f == "isnotfolder") || v["IsFolder"].as_bool() != Some(true))
                && is_played.is_none_or(|p| played(v) == p)
                && (years.is_empty() || v["ProductionYear"].as_i64().is_some_and(|y| years.contains(&y)))
                && (genres.is_empty()
                    || v["Genres"].as_array().is_some_and(|g| g.iter().any(|x| genres.contains(&x.as_str().unwrap_or("").to_ascii_lowercase()))))
                && term.as_ref().is_none_or(|t| name.contains(t.as_str()) || v["OriginalTitle"].as_str().is_some_and(|o| o.to_lowercase().contains(t.as_str())))
                && starts.as_ref().is_none_or(|s| sort_name.starts_with(s.as_str()))
                && starts_or_greater.as_ref().is_none_or(|s| sort_name.as_str() >= s.as_str())
                && less_than.as_ref().is_none_or(|s| sort_name.as_str() < s.as_str())
        })
        .collect()
}

pub fn page(items: Vec<Value>, q: &Q) -> (Vec<Value>, usize, usize) {
    let total = items.len();
    let start = q.int("startindex").unwrap_or(0).max(0) as usize;
    let limit = q.int("limit").filter(|l| *l >= 0).map(|l| l as usize).unwrap_or(usize::MAX);
    (items.into_iter().skip(start).take(limit).collect(), total, start)
}

pub fn query_result(items: Vec<Value>, total: usize, start: usize) -> Response {
    json(json!({ "Items": items, "TotalRecordCount": total, "StartIndex": start }))
}

pub fn empty_result() -> Response {
    query_result(Vec::new(), 0, 0)
}

fn unsupported_filter(q: &Q) -> bool {
    ["personids", "person", "studioids", "studios", "genreids", "tags", "artistids", "albumids", "officialratings"]
        .iter()
        .any(|k| q.get(k).is_some())
        || q.bool("isfavorite") == Some(true)
        || q.list("filters").iter().any(|f| f.eq_ignore_ascii_case("isfavorite") || f.eq_ignore_ascii_case("likes"))
        || q.list("mediatypes").iter().any(|m| !m.eq_ignore_ascii_case("video"))
}

async fn person_items(st: &Shared, auth: &Auth, q: &Q) -> ApiResult<Vec<Value>> {
    let mut out = Vec::new();
    for pid in q.list("personids") {
        match Id::decode(&pid) {
            Some(Id { kind: Kind::MoviePerson, parent, .. }) => out.push(items::movie(&ctx(st, auth), &st.ciniva.movie(&auth.token, parent).await?)),
            Some(Id { kind: Kind::ShowPerson, parent, .. }) => out.push(items::show(&ctx(st, auth), &st.ciniva.show(&auth.token, parent).await?, None)),
            _ => {}
        }
    }
    Ok(out)
}

/// Items for `/Items` and `/Users/{id}/Items` before filtering and paging.
async fn source(st: &Shared, auth: &Auth, q: &Q) -> ApiResult<Vec<Value>> {
    let token = auth.token.as_str();
    let types: Vec<String> = q.list("includeitemtypes").iter().map(|s| s.to_ascii_lowercase()).collect();
    let wants = |t: &str| types.is_empty() || types.iter().any(|x| x == t);
    let recursive = q.bool("recursive").unwrap_or(false);

    let ids = q.list("ids");
    if !ids.is_empty() {
        let mut out = Vec::new();
        for id in ids {
            if let Ok(r) = resolve(st, token, &id).await {
                out.push(item_value(st, auth, &r).await?);
            }
        }
        return Ok(out);
    }
    if q.get("personids").is_some() {
        return person_items(st, auth, q).await;
    }
    if unsupported_filter(q) {
        return Ok(Vec::new());
    }
    if q.list("filters").iter().any(|f| f.eq_ignore_ascii_case("isresumable")) {
        return resume_items(st, auth).await;
    }

    let parent = q.get("parentid").map(str::to_string);
    let parent_id = parent.as_deref().and_then(Id::decode);
    match parent_id.map(|p| p.kind) {
        Some(Kind::MoviesView) => Ok(if wants("movie") { movie_cards(st, auth, &st.ciniva.movies(token, "title").await?) } else { Vec::new() }),
        Some(Kind::ShowsView) => {
            if recursive && types.iter().any(|t| t == "episode") {
                all_episodes(st, auth).await
            } else if recursive && types.iter().any(|t| t == "season") {
                all_seasons(st, auth).await
            } else {
                Ok(show_cards(st, auth, &st.ciniva.shows(token, "title").await?))
            }
        }
        Some(Kind::Series) => {
            let show = st.ciniva.show(token, parent_id.unwrap().parent).await?;
            if recursive && (types.is_empty() || types.iter().any(|t| t == "episode")) {
                let eps = season_episodes(st, token, &show, &season_order(&show)).await?;
                let mut out: Vec<Value> = eps.iter().map(|e| items::episode(&ctx(st, auth), &show, e)).collect();
                if types.is_empty() {
                    out.extend(show.seasons.iter().map(|c| items::season(&ctx(st, auth), &show, c)));
                }
                Ok(out)
            } else {
                Ok(show.seasons.iter().map(|c| items::season(&ctx(st, auth), &show, c)).collect())
            }
        }
        Some(Kind::Season) => {
            let p = parent_id.unwrap();
            let (show, eps) = tokio::try_join!(st.ciniva.show(token, p.parent), st.ciniva.season(token, p.parent, p.season))?;
            Ok(eps.iter().filter(|e| !e.files.is_empty()).map(|e| items::episode(&ctx(st, auth), &show, e)).collect())
        }
        Some(Kind::Root) => Ok(views(st, auth).await?),
        Some(_) => Ok(Vec::new()),
        None if parent.is_some() => Ok(Vec::new()),
        None => {
            if !recursive && types.is_empty() && q.get("searchterm").is_none() {
                return views(st, auth).await;
            }
            let mut out = Vec::new();
            if wants("movie") {
                out.extend(movie_cards(st, auth, &st.ciniva.movies(token, "title").await?));
            }
            if wants("series") {
                out.extend(show_cards(st, auth, &st.ciniva.shows(token, "title").await?));
            }
            if types.iter().any(|t| t == "season") {
                out.extend(all_seasons(st, auth).await?);
            }
            if types.iter().any(|t| t == "episode") {
                out.extend(all_episodes(st, auth).await?);
            }
            Ok(out)
        }
    }
}

fn adjacent(items: Vec<Value>, to: &str) -> Vec<Value> {
    let to = to.replace('-', "").to_ascii_lowercase();
    match items.iter().position(|v| v["Id"].as_str() == Some(to.as_str())) {
        Some(i) => items.into_iter().skip(i.saturating_sub(1)).take(if i == 0 { 2 } else { 3 }).collect(),
        None => Vec::new(),
    }
}

pub async fn list_items(State(st): State<Shared>, auth: Auth, q: Q) -> ApiResult {
    let mut all = filter(source(&st, &auth, &q).await?, &q);
    if q.list("sortby").is_empty() && q.get("parentid").is_none() && q.list("ids").is_empty() {
        all.sort_by(|a, b| cmp_by(a, b, "sortname"));
    }
    sort(&mut all, &q);
    if let Some(to) = q.get("adjacentto") {
        all = adjacent(all, to);
    }
    let (page, total, start) = page(all, &q);
    let page = enrich(&st, &auth, page).await;
    Ok(query_result(page, total, start))
}

pub async fn views(st: &Shared, auth: &Auth) -> ApiResult<Vec<Value>> {
    let (movies, shows) = tokio::try_join!(st.ciniva.movies(&auth.token, "title"), st.ciniva.shows(&auth.token, "title"))?;
    Ok(vec![items::view(&ctx(st, auth), Kind::MoviesView, Some(movies.len())), items::view(&ctx(st, auth), Kind::ShowsView, Some(shows.len()))])
}

pub async fn user_views(State(st): State<Shared>, auth: Auth) -> ApiResult {
    let v = views(&st, &auth).await?;
    let n = v.len();
    Ok(query_result(v, n, 0))
}

pub async fn get_item(State(st): State<Shared>, auth: Auth, Path(params): Path<Vec<(String, String)>>) -> ApiResult {
    let id = params.iter().find(|(k, _)| k == "id").map(|(_, v)| v.clone()).ok_or_else(not_found)?;
    let r = resolve(&st, &auth.token, &id).await?;
    Ok(json(item_value(&st, &auth, &r).await?))
}

pub async fn root_folder(State(st): State<Shared>, auth: Auth) -> ApiResult {
    Ok(json(items::root(&ctx(&st, &auth))))
}

pub async fn latest(State(st): State<Shared>, auth: Auth, q: Q) -> ApiResult {
    let token = auth.token.as_str();
    let limit = q.int("limit").unwrap_or(16).clamp(1, 100) as usize;
    let parent = q.get("parentid").and_then(Id::decode).map(|p| p.kind);
    let types: Vec<String> = q.list("includeitemtypes").iter().map(|s| s.to_ascii_lowercase()).collect();
    let movies = matches!(parent, None | Some(Kind::MoviesView)) && (types.is_empty() || types.iter().any(|t| t == "movie"));
    let shows = matches!(parent, None | Some(Kind::ShowsView)) && (types.is_empty() || types.iter().any(|t| t == "series" || t == "episode"));
    let mut out = Vec::new();
    if movies {
        out.extend(movie_cards(&st, &auth, &st.ciniva.movies(token, "added").await?));
    }
    if shows {
        out.extend(show_cards(&st, &auth, &st.ciniva.shows(token, "added").await?));
    }
    if q.bool("isplayed") == Some(false) {
        out.retain(|v| !played(v));
    }
    out.sort_by(|a, b| cmp_by(b, a, "datecreated"));
    out.truncate(limit);
    Ok(json(Value::Array(enrich(&st, &auth, out).await)))
}

async fn resume_items(st: &Shared, auth: &Auth) -> ApiResult<Vec<Value>> {
    let token = auth.token.as_str();
    let home = st.ciniva.home(token).await?;
    let mut out = Vec::new();
    for c in home.continue_watching {
        if let Some(mid) = c.movie_id {
            if let Ok(m) = st.ciniva.movie(token, mid).await {
                out.push(items::movie(&ctx(st, auth), &m));
            }
        } else if let Some(sid) = c.show_id {
            let season = c
                .subtitle
                .as_deref()
                .and_then(|s| s.strip_prefix('S'))
                .and_then(|s| s.split(|ch: char| !ch.is_ascii_digit()).next())
                .and_then(|n| n.parse::<i64>().ok());
            let Ok(show) = st.ciniva.show(token, sid).await else { continue };
            let seasons = match season {
                Some(n) => vec![n],
                None => season_order(&show),
            };
            let eps = season_episodes(st, token, &show, &seasons).await?;
            if let Some(e) = eps.iter().find(|e| e.files.iter().any(|f| f.id == c.file_id)) {
                out.push(items::episode(&ctx(st, auth), &show, e));
            }
        }
    }
    Ok(out)
}

pub async fn resume(State(st): State<Shared>, auth: Auth, q: Q) -> ApiResult {
    let mut all = resume_items(&st, &auth).await?;
    match q.get("parentid").and_then(Id::decode).map(|p| p.kind) {
        Some(Kind::MoviesView) => all.retain(|v| v["Type"] == "Movie"),
        Some(Kind::ShowsView) => all.retain(|v| v["Type"] == "Episode"),
        _ => {}
    }
    if q.list("mediatypes").iter().any(|m| !m.eq_ignore_ascii_case("video")) && !q.list("mediatypes").iter().any(|m| m.eq_ignore_ascii_case("video")) {
        all.clear();
    }
    let all = filter(all, &q);
    let (page, total, start) = page(all, &q);
    Ok(query_result(page, total, start))
}

pub async fn next_up(State(st): State<Shared>, auth: Auth, q: Q) -> ApiResult {
    let token = auth.token.as_str();
    let series = q.get("seriesid").and_then(Id::decode).filter(|i| i.kind == Kind::Series);
    if let Some(p) = q.get("parentid").and_then(Id::decode) {
        if p.kind == Kind::MoviesView {
            return Ok(empty_result());
        }
    }
    let ids: Vec<i64> = match series {
        Some(s) => vec![s.parent],
        None => st
            .ciniva
            .shows(token, "added")
            .await?
            .into_iter()
            .filter(|c| c.watched_episodes > 0 && c.watched_episodes < c.episodes)
            .map(|c| c.id)
            .take(30)
            .collect(),
    };
    let rewatching = q.bool("enablerewatching").unwrap_or(false);
    let mut out = Vec::new();
    for id in ids {
        let show = st.ciniva.show(token, id).await?;
        let regular: Vec<i64> = season_order(&show).into_iter().filter(|n| *n > 0).collect();
        let eps = season_episodes(&st, token, &show, &regular).await?;
        let watched = |e: &Episode| e.files.iter().any(|f| f.progress.watched.unwrap_or(false));
        let last = eps.iter().rposition(watched);
        let next = match last {
            Some(i) => eps.iter().skip(i + 1).find(|e| rewatching || !watched(e)),
            None if series.is_some() => eps.iter().find(|e| !watched(e)),
            None => None,
        };
        if let Some(e) = next {
            out.push(items::episode(&ctx(&st, &auth), &show, e));
        }
    }
    let (page, total, start) = page(out, &q);
    Ok(query_result(page, total, start))
}

pub async fn seasons(State(st): State<Shared>, auth: Auth, Path(id): Path<String>, q: Q) -> ApiResult {
    let show_id = Id::decode(&id).filter(|i| i.kind == Kind::Series).ok_or_else(not_found)?.parent;
    let show = st.ciniva.show(&auth.token, show_id).await?;
    let mut list: Vec<Value> = show.seasons.iter().map(|c| items::season(&ctx(&st, &auth), &show, c)).collect();
    if q.bool("isspecialseason") == Some(false) {
        list.retain(|v| v["IndexNumber"] != 0);
    }
    let n = list.len();
    Ok(query_result(list, n, 0))
}

pub async fn episodes(State(st): State<Shared>, auth: Auth, Path(id): Path<String>, q: Q) -> ApiResult {
    let show_id = Id::decode(&id).filter(|i| i.kind == Kind::Series).ok_or_else(not_found)?.parent;
    let show = st.ciniva.show(&auth.token, show_id).await?;
    let wanted: Vec<i64> = match (q.get("seasonid").and_then(Id::decode), q.int("season")) {
        (Some(s), _) if s.kind == Kind::Season => vec![s.season],
        (_, Some(n)) => vec![n],
        _ => season_order(&show),
    };
    let eps = season_episodes(&st, &auth.token, &show, &wanted).await?;
    let mut list: Vec<Value> = eps.iter().map(|e| items::episode(&ctx(&st, &auth), &show, e)).collect();
    if let Some(start) = q.get("startitemid").map(|s| s.replace('-', "").to_ascii_lowercase()) {
        if let Some(i) = list.iter().position(|v| v["Id"].as_str() == Some(start.as_str())) {
            list.drain(..i);
        }
    }
    if let Some(to) = q.get("adjacentto") {
        list = adjacent(list, to);
    }
    let list = filter(list, &q);
    let (page, total, start) = page(list, &q);
    Ok(query_result(page, total, start))
}

pub async fn counts(State(st): State<Shared>, auth: Auth) -> ApiResult {
    let (movies, shows) = tokio::try_join!(st.ciniva.movies(&auth.token, "title"), st.ciniva.shows(&auth.token, "title"))?;
    let episodes: i64 = shows.iter().map(|s| s.episodes).sum();
    Ok(json(json!({
        "MovieCount": movies.len(), "SeriesCount": shows.len(), "EpisodeCount": episodes,
        "ArtistCount": 0, "ProgramCount": 0, "TrailerCount": 0, "SongCount": 0, "AlbumCount": 0,
        "MusicVideoCount": 0, "BoxSetCount": 0, "BookCount": 0, "ItemCount": movies.len() as i64 + shows.len() as i64 + episodes,
    })))
}

pub async fn ancestors(State(st): State<Shared>, auth: Auth, Path(id): Path<String>) -> ApiResult {
    let cx = ctx(&st, &auth);
    let root = items::root(&cx);
    let out = match resolve(&st, &auth.token, &id).await? {
        Resolved::Movie(_) => vec![items::view(&cx, Kind::MoviesView, None), root],
        Resolved::Series(_) => vec![items::view(&cx, Kind::ShowsView, None), root],
        Resolved::Season(s, _) => vec![items::show(&cx, &s, None), items::view(&cx, Kind::ShowsView, None), root],
        Resolved::Episode(s, e) => {
            let season = s.seasons.iter().find(|c| c.season_number == e.season_number).map(|c| items::season(&cx, &s, c));
            season.into_iter().chain([items::show(&cx, &s, None), items::view(&cx, Kind::ShowsView, None), root]).collect()
        }
        Resolved::View(_) => vec![root],
        _ => Vec::new(),
    };
    Ok(json(Value::Array(out)))
}

pub async fn filters(State(st): State<Shared>, auth: Auth, q: Q) -> ApiResult {
    let mut genres: Vec<String> = Vec::new();
    let mut years: Vec<i64> = Vec::new();
    let parent = q.get("parentid").and_then(Id::decode).map(|p| p.kind);
    if matches!(parent, None | Some(Kind::MoviesView)) {
        for m in st.ciniva.movies(&auth.token, "title").await? {
            years.extend(m.year);
        }
    }
    if matches!(parent, None | Some(Kind::ShowsView)) {
        for s in st.ciniva.shows(&auth.token, "title").await? {
            years.extend(s.year);
        }
    }
    genres.sort();
    years.sort_unstable();
    years.dedup();
    Ok(json(json!({ "Genres": genres, "Tags": [], "OfficialRatings": [], "Years": years })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, year: i64, added: &str) -> Value {
        json!({ "Name": name, "SortName": items::sort_name(name), "ProductionYear": year, "DateCreated": added, "Type": "Movie",
                "Id": name, "UserData": { "Played": false, "PlaybackPositionTicks": 0 } })
    }

    #[test]
    fn sorts_filters_and_pages() {
        let all = vec![item("The Zoo", 2001, "2026-01-02"), item("Alpha", 1999, "2026-01-03"), item("Beta", 2010, "2026-01-01")];
        let q = Q::parse(Some("SortBy=ProductionYear,SortName&SortOrder=Descending&StartIndex=1&Limit=1"));
        let mut v = filter(all.clone(), &q);
        sort(&mut v, &q);
        let (p, total, start) = page(v, &q);
        assert_eq!((total, start), (3, 1));
        assert_eq!(p[0]["Name"], "The Zoo");
        let q = Q::parse(Some("SearchTerm=ALP"));
        assert_eq!(filter(all.clone(), &q).len(), 1);
        let q = Q::parse(Some("NameStartsWith=z"));
        assert_eq!(filter(all.clone(), &q)[0]["Name"], "The Zoo");
        let q = Q::parse(Some("IncludeItemTypes=Series"));
        assert!(filter(all, &q).is_empty());
    }

    #[test]
    fn adjacent_items() {
        let all: Vec<Value> = (1..=4).map(|i| json!({ "Id": format!("e{i}") })).collect();
        let ids = |v: Vec<Value>| v.iter().map(|x| x["Id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(ids(adjacent(all.clone(), "e2")), ["e1", "e2", "e3"]);
        assert_eq!(ids(adjacent(all.clone(), "e1")), ["e1", "e2"]);
        assert_eq!(ids(adjacent(all, "e4")), ["e3", "e4"]);
    }
}
