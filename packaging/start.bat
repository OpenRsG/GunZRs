@echo off
rem Starts GunZRs on its main menu; extra arguments go to gunz-play (start.bat Mansion --bots 3).
rem The game is found through Steam; set GUNZ_GAME=DIR to point at another install.
cd /d "%~dp0"
rem renders the maps' pictures for the menu and the loading screen; does nothing once they exist
gunz.exe --previews
start "" gunz-play.exe %*
