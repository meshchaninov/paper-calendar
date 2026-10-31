#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
static void (*timer_cb)(void);
static int updates;
static int partials;
static int orientation;
static int panel_type=2;
static int sleep_mode=1,sleeps=0,wifi=1,light=50;
static int clipped;
static int cleared;
void InkViewMain(int (*handler)(int,int,int)) {
    handler(21,0,0); handler(23,0,0);
    for (int i=0;i<100;i++) {usleep(100000);if(timer_cb)timer_cb();}
    handler(29,100,50); // Manual refresh, unchanged demo frame.
    for (int i=0;i<30;i++) {usleep(100000);if(timer_cb)timer_cb();}
    handler(29,1600,1000);
    handler(29,1600,280); // Timed event opens details.
    handler(29,1600,55);   // Back restores the list.
    handler(29,300,200);
    handler(29,1140,50);
    int prior=updates+partials;
    if(handler(25,0x19,0)!=1)exit(10); // Next month, consume physical key.
    for (int i=0;i<10;i++) {usleep(100000);if(timer_cb)timer_cb();}
    if(updates+partials<=prior)exit(11);
    prior=updates+partials;
    handler(26,0x19,0); handler(28,0x19,0); // Release/repeat must not page twice.
    if(updates+partials!=prior)exit(12);
    if(handler(25,0x18,0)!=1)exit(13); // Previous month.
    for (int i=0;i<10;i++) {usleep(100000);if(timer_cb)timer_cb();}
    if(updates+partials<=prior)exit(14);
    handler(29,1140,50); // Today restores actual date and month.
    handler(24,0,0); panel_type=2; clipped=0;
    handler(23,0,0);
    for (int i=0;i<10;i++) {usleep(100000);if(timer_cb)timer_cb();}
    handler(22,0,0);
    fprintf(stderr,"STUB: full updates=%d, partial updates=%d\n",updates,partials);
    if(updates<2 || partials<5 || sleeps<1)exit(2);
    fprintf(stderr,"STUB: sleep calls=%d, wifi=%d, light=%d\n",sleeps,wifi,light);
}
void SetPanelType(int value){panel_type=value;}
int GetPanelType(void){return panel_type;}
void SetClip(int x,int y,int width,int height){clipped=(x>=0&&y>=0&&width>0&&height>0&&x+width<=1872&&y+height<=1404);}
void ClearScreen(void){cleared=1;}
void SetOrientation(int value){if(panel_type!=0){fprintf(stderr,"panel still enabled before orientation\n");exit(4);}orientation=value;}
int GetOrientation(void){return orientation;}
int ScreenWidth(void){return 1872;}
int ScreenHeight(void){return 1404;}
void *LoadPNG(const char *path,int dither){FILE *f=fopen(path,"rb");if(!f)return NULL;fclose(f);return malloc(1);}
void DrawBitmap(int x,int y,const void *bitmap){
 if(!bitmap)exit(3);
 const unsigned short *header=bitmap;
 if(header[0]==0 || header[1]==0 || header[2]!=8 || header[3]!=header[0] || x<0 || y<0 || x+header[0]>1872 || y+header[1]>1404)exit(15);
 if(panel_type!=0 || !clipped || !cleared){fprintf(stderr,"invalid full-screen drawing state\n");exit(5);}
 cleared=0;
}
void FullUpdate(void){updates++;}
void FillArea(int x,int y,int width,int height,int color){if(x<0||y<0||width<=0||height<=0||x+width>1872||y+height>1404)exit(6);cleared=1;}
void PartialUpdate(int x,int y,int width,int height){if(x<0||y<0||width<=0||height<=0||x+width>1872||y+height>1404)exit(7);partials++;}
void SetHardTimer(const char *name,void (*callback)(void),int ms){timer_cb=callback;}
void BanSleep(int seconds){}
void CloseApp(void){}

int GoSleep(int ms,int deep){if(!sleep_mode||wifi||ms<1000)exit(8);sleeps++;usleep(120000);return 0;}
void iv_sleepmode(int enabled){sleep_mode=enabled;}
int GetSleepmode(void){return sleep_mode;}
int IsTaskActive(void){return 1;}
int get_keylock(void){return 0;}
int IsCharging(void){return 0;}
int GetBatteryPower(void){return 85;}
int WiFiPower(int state){wifi=state;return state;}
int NetConnectSilent(const char *name){wifi=1;return 0;}
int NetDisconnect(void){wifi=0;return 0;}
int GetFrontlightState(void){return light;}
void SetFrontlightStateEx(int state,int temporary){light=state;}
void SetAutoPowerOff(int state){}
