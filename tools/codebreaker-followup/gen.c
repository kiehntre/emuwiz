#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include "cb2_crypto.h"
static uint64_t s=0x9E3779B97F4A7C15ULL;
static uint32_t rnd(void){ s^=s<<13; s^=s>>7; s^=s<<17; return (uint32_t)(s>>16); }
/* output: "<mode> <n>" then n lines "plainA plainV encA encV" */
int main(int argc,char**argv){
  int mode=atoi(argv[1]); /* 0=v1 stream (reset), 1=common v7 */
  int streams=atoi(argv[2]); int prof=argc>3?atoi(argv[3]):2;
  for(int t=0;t<streams;t++){
    int n=4+rnd()%60; if(prof==5 && n<6)n=6; 
    printf("%d %d\n",mode,n);
    /* encrypt pass */
    uint32_t pa[80],pv[80],ea[80],ev[80];
    for(int i=0;i<n;i++){
      pa[i]=rnd(); pv[i]=rnd();
      uint32_t r=rnd()%12;
      if((prof==0 || prof>=3) && (r==0||r==1)) r=5;
      if(prof==1){ if(r<2) r=5; if(i==0){ pa[i]=0xBEEFC0DE; pv[i]=getenv("ZV")?0:((rnd()%2)?0:rnd()); continue; } }
      if(r==0){pa[i]=0xBEEFC0DE; if(rnd()%4==0)pv[i]=0;}
      else if(r==1 && i>0){pa[i]=0xBEEFC0DF;}
      else if(r<8){pa[i]=(rnd()%3<<28)|(pa[i]&0x01FFFFFF);} /* 0/1/2 direct */
    }
    if(prof>=3){
      pa[0]=0xBEEFC0DE; pv[0]=0;
      pa[1]=prof==3?0xBEEFC0DE:0xBEEFC0DF; pv[1]=0xB16B00B5;
      pa[2]=prof==3?0xBEEFC0DE:0xBEEFC0DF; pv[2]=0;
      pa[3]=0x201F6024; pv[3]=0;
    }
    if(prof==5){
      pa[0]=0xBEEFC0DE; pv[0]=0;
      pa[1]=0xBEEFC0DF; pv[1]=0xB16B00B5;
      pa[2]=0x01234567; pv[2]=0x89ABCDEF;
      pa[3]=0xBEEFC0DF; pv[3]=0;
      pa[4]=0x01234567; pv[4]=0x89ABCDEF;
      pa[5]=0x201F6024; pv[5]=0;
    }
    if(mode==1) cb_set_common_v7(); else cb_reset();
    for(int i=0;i<n;i++){ ea[i]=pa[i]; ev[i]=pv[i]; cb_encrypt_code(&ea[i],&ev[i]); }
    for(int i=0;i<n;i++) printf("%08X %08X %08X %08X\n",pa[i],pv[i],ea[i],ev[i]);
    /* C decrypt pass for check */
    if(mode==1) cb_set_common_v7(); else cb_reset();
    for(int i=0;i<n;i++){ uint32_t a=ea[i],v=ev[i]; cb_decrypt_code(&a,&v); printf("C %08X %08X\n",a,v); }
  }
}
