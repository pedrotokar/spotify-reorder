use rspotify::prelude::*;
use rspotify::{scopes, AuthCodeSpotify, Credentials, OAuth, ClientResult, ClientError, Token};
use rspotify::model::{SimplifiedPlaylist, PlaylistItem, PlaylistId, PlayableItem, Market, FullEpisode, FullTrack};
use rspotify::clients::pagination::Paginator;
use std::{io, fmt, thread, time};
use std::error::Error;

//This is an error enum, so i can have my personalized errors and to be able to put all of them in
//the function annotation inside the result.
#[derive(Debug)]
enum OrderingError {
    SpotifyError(ClientError),
    EpisodeInPlaylist(FullEpisode),
    LocalMusicInPlaylist(FullTrack),
    EmptyArgOnMusic(String, String),
}
impl fmt::Display for OrderingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EpisodeInPlaylist(episode) => write!(formatter, "The playlist cannot be sorted, \
            as it has a podcast: {}", episode.name),
            Self::LocalMusicInPlaylist(music) => write!(formatter, "The playlist cannot be sorted, \
            as it has a music from your storage: {}", music.name),
            Self::EmptyArgOnMusic(param, music) => write!(formatter, "The sorting process failed \
            because the music {} param {} is blank", music, param),
            Self::SpotifyError(original_error) => write!(formatter, "{}", original_error),
        }
    }
}
impl From<ClientError> for OrderingError {
    fn from(error: ClientError) -> Self {
        Self::SpotifyError(error)
    }
}
impl Error for OrderingError {}

#[derive(Debug)]
struct CachedLoginError;
impl fmt::Display for CachedLoginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result{
        write!(formatter, "Unable to login with cached token.")
    }
}
impl Error for CachedLoginError {}

//This function will get a cached token from the storage. If it fails reading or the token does not
//exist, it'll return an error, indicating that the program should ask for the authorization. If its
//successfull, it'll return only a ok, since the spotify object will aready be with the token refreshed
fn read_cached_token(spotify: &mut AuthCodeSpotify) -> Result<(), CachedLoginError>{
    let token = match Token::from_cache(".token_cache"){
        Ok(result) => Some(result),
        Err(_) => return Err(CachedLoginError),
    };
    match spotify.token.lock(){
        Ok(mut token_ref) => *token_ref = token,
        Err(_) => return Err(CachedLoginError),
    }
    match spotify.refresh_token(){
        Ok(_) => {},
        Err(_) => return Err(CachedLoginError),
    }
    Ok(())
}

//This function get a list of all of the user's playlists, filtered by only the ones they own.
//It can error when the lib errors by some reason (since the iterator return the playlists inside
//a result that must be checked)
fn get_user_playlists(spotify: &AuthCodeSpotify) -> Result<Vec<SimplifiedPlaylist>, OrderingError> {
    let current_user_id = String::from(spotify.current_user()?.id.id());
    let mut user_owned_playlists: Vec<SimplifiedPlaylist> = Vec::new();
    for playlist in spotify.current_user_playlists(){
        let playlist = playlist?;
        if playlist.owner.id.id() == current_user_id {
            user_owned_playlists.push(playlist);
        }
    }
    Ok(user_owned_playlists)
}

//This function get two vectors, one having all the strings in the original order of the playlist
//and one with them ordered. To be honest, I dont know if I should make it only return an unordered
//vector and them order it outside the function... Or return the ordered list as a slice, since
//it won't be muted in the program.... But now it is what it is.
//It can error when the playlist has a podcast or a local music, and also when a music have a blank
//parameter.
//TODO: Implement ordering customization?
fn get_music_list(
    playlist_iterable: Paginator<ClientResult<PlaylistItem>>
) -> Result<(Vec<String>, Vec<String>), OrderingError> {
    let mut unordered_music_list: Vec<String> = Vec::new();
    for music in playlist_iterable{
        let music = match music?.track{
            Some(track) => match track{
                PlayableItem::Track(music) => music,
                PlayableItem::Episode(podcast) => return Err(OrderingError::EpisodeInPlaylist(podcast))},
            None => panic!("Since I don't know why a playlist item may not have anything associated \
                            with it, I'll leave this without further handling."),
        };
        if music.is_local{
            return Err(OrderingError::LocalMusicInPlaylist(music));
        }
        let mut music_label = String::new();
        music_label.push_str(&music.artists[0].name);
        music_label.push('-');
        music_label.push_str(
            &music.album.release_date.ok_or(
                OrderingError::EmptyArgOnMusic(String::from("album.release_date"), music.name.clone())
            )?
        );
        let release_date_precision = music.album.release_date_precision.ok_or(
            OrderingError::EmptyArgOnMusic(String::from("album.release_date_precision"), music.name.clone())
        )?;
        if release_date_precision == "year"{
            music_label.push_str("-01-01");
        } else if release_date_precision == "month"{
            music_label.push_str("-01");
        }
        music_label.push('-');
        music_label.push_str(&music.album.name);
        music_label.push('-');
        music_label.push_str(&format!("{:0>6}", music.disc_number));
        music_label.push('-');
        music_label.push_str(&format!("{:0>6}", music.track_number));
        music_label.push('-');
        music_label.push_str(&music.name);
        unordered_music_list.push(music_label);
    }

    let mut ordered_music_list = unordered_music_list.clone().to_vec();
    ordered_music_list.sort_unstable();
    Ok((unordered_music_list, ordered_music_list))
}

//This function makes all the operations on the spotify side, and only returns a empty tuple if success,
//or an error saying why it failed (errors in the lib/api size). It consumes both vectors.
fn reorder_musics(
    spotify: &AuthCodeSpotify,
    playlist_id: PlaylistId,
    ordered_music_list: Vec<String>,
    unordered_music_list: &mut Vec<String>
) -> Result<(), OrderingError> {
    let mut ordered_list_index: usize = 0;
    let mut unordered_list_index: usize;
    let mut music_sequence_count: usize = 0;
    let music_len: usize = ordered_music_list.len();
    let delay_time: usize = ordered_music_list.len()*2;

    while ordered_list_index < music_len{
        let current_music: &str = &ordered_music_list[ordered_list_index];
        unordered_list_index = unordered_music_list.iter()
                                                    .position(|x| x == &current_music)
                                                    .unwrap(); //This should never error!
        if ordered_list_index == unordered_list_index {
            ordered_list_index += 1;
            continue;
        }
        if ordered_list_index+1 != music_len && unordered_list_index+1 != music_len{
            while &ordered_music_list[ordered_list_index + 1 + music_sequence_count] ==
                  &unordered_music_list[unordered_list_index + 1 + music_sequence_count] {
                music_sequence_count += 1;
                if ordered_list_index + music_sequence_count + 1 == music_len ||
                unordered_list_index + music_sequence_count + 1 == music_len {break;}
            }
        }
        println!("Currently working on {}", current_music);
        spotify.playlist_reorder_items(
            playlist_id.clone(),
            Some(unordered_list_index as i32),
            Some(ordered_list_index as i32),
            Some(music_sequence_count as u32 + 1),
            None
        )?;
        //Original idea was moving each music individually, removing and adding again, till I found
        //about the drain method, which can help when moving multiple musics. I don't know if this can be
        //a bad move for moving only one element, and I need to further research about that.
        let moved_musics = unordered_music_list.drain(
            unordered_list_index..unordered_list_index+music_sequence_count+1
        ).collect::<Vec<String>>();
        for (position, item) in moved_musics.iter().enumerate(){
            unordered_music_list.insert(ordered_list_index + position, item.to_string());
        }
        ordered_list_index += 1;
        music_sequence_count = 0;
        thread::sleep(time::Duration::from_millis(delay_time as u64)); //Idk if I should let this here,
        //in the past Spotify sometimes ignored the changes I made if I didn't used this delay, but now
        //it seems stable, so that's another thing I need to research about.
    }
    Ok(())
}


fn main(){
    //Code to authenticate in spotify.
    //I've commited some war crimes in this part a.k.a nested matchs but now I really don't know
    //how to make it better.
    let credentials = Credentials::from_env().unwrap();
    let oauth = OAuth::from_env(
        scopes!("playlist-read-private", "playlist-modify-private", "playlist-modify-public")
    ).unwrap();
    let mut spotify = AuthCodeSpotify::new(credentials, oauth);
    match read_cached_token(&mut spotify){
        Ok(_) => {}, //Already logged, do nothing
        Err(_) => { //No cached login, asks for authentication
            let url = spotify.get_authorize_url(false).expect("Unknown error.");
            match spotify.prompt_for_token(&url){
                Ok(_) => match spotify.token.lock(){ //Logged in,just trying to cache token
                    //I unwrap the as_ref here since the spotify object shouldn't be without a token...
                    Ok(token_ref) => match token_ref.as_ref().unwrap().write_cache(".token_cache"){
                        Ok(_) => println!("Successfully cached token."),
                        Err(_) => println!("Couldn't cache the token"),
                    },
                    Err(_) => println!("Couldn't cache the token"),
                },
                Err(error) => panic!("Failed to authenticate into spotify: {}", error),
            }
        },
    }
    println!("I've sucefully logged in as {}.", spotify.current_user().unwrap().display_name.unwrap());

    //Code to get and print playlists, also handle whe a user own no playlist.
    let user_owned_playlists = match get_user_playlists(&spotify){
        Ok(vector) => vector,
        Err(error) => panic!("{}", error),
    };
    if user_owned_playlists.len() == 0 {
        panic!("You have no playlists to order."); //I definivelly need to rework on this (and also in exiting in general)
    }
    println!("Your current owned playlists are:\n");
    for (number, playlist) in user_owned_playlists.iter().enumerate(){
        println!("{} - {}", number + 1, playlist.name);
    }

    //This part about which playlist should be reordered.
    let mut pl_index_string: String;
    let mut pl_index: usize;
    loop{
        println!("Please say which playlist you want to sort.");
        pl_index_string = String::new();
        io::stdin().read_line(&mut pl_index_string).expect("Something bad happend while reading input.");
        pl_index = match pl_index_string.trim().parse::<usize>() {
            Ok(numero) => numero,
            Err(_) => {
                println!("You didn't typed a number.");
                continue;
            },
        };
        if pl_index < 1 || pl_index > user_owned_playlists.len(){
            println!("The number you wrote doesn't ressemble any playlist in the list.");
            continue;
        } else {
            break;
        }
    }
    //This thing with two vars was made because a variable gets dropped when it goes out
    //of scope, and by so I wouldnt get pl_index as a int if it were shadowed inside the scope.

    //Now, the code get the playlist items.
    let playlist_id = user_owned_playlists[pl_index - 1].id.clone();
    let playlist = spotify.playlist_items(
        playlist_id.clone(),
        None, //I first tryed to use a scopes string, but that got me only headache and a bad sleep night.
        Some(Market::FromToken)
    );
    let (mut unordered_music_list, ordered_music_list) = match get_music_list(playlist){
        Ok(results) => results,
        Err(error) => panic!("{}", error),
    };

    //Asks confirmation and then does the black magic.
    println!(
        "You've chosen playlist {} (with ID {}) containing {} musics. \
        Proceed with the sort? Type yes to proceed.",
        &user_owned_playlists[pl_index - 1].name,
        playlist_id.id(),
        ordered_music_list.len()
    );
    let mut user_confirmation = String::new();
    io::stdin().read_line(&mut user_confirmation).unwrap();
    if user_confirmation.trim() != "yes"{
        println!("Operation cancelled.");
    }
    else {
        match reorder_musics(
            &spotify,
            playlist_id,
            ordered_music_list,
            &mut unordered_music_list
        ){
            Ok(_) => println!("Sucessfull operation. The playlist is now sorted"),
            Err(error) => println!("{}", error),
        }
    }
}

//const PLAYLIST_REQUIRED_FIELDS: &str =
//    "href,limit,next,offset,previous,total,items(track(album(album_type,id,release_date,\
//    release_date_precision,name,artists),artists(name,popularity,genre),disc_number,linked_from,\
//    name,track_number))";
