MEMORY
{
    /* 0x08003000-0x0800F000 App (48K)
       0x0800F000-0x0800FBFF remap banks
       0x0800FC00 Novation bootloader settings page
       0x08000000-0x08002FFF bootloader */
    FLASH : ORIGIN = 0x08003000, LENGTH = 48K
    RAM   : ORIGIN = 0x20000000, LENGTH = 20K
}
