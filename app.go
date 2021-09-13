package main

import (
	"fmt"
	"log"
	"os"
	"sync"
	"time"

	"github.com/labstack/echo/v4"
	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/menu"
	"github.com/wailsapp/wails/v2/pkg/menu/keys"
	"github.com/wailsapp/wails/v2/pkg/options/dialog"
)

const (
	ConcurrentIncomeURL = 1
)

// app struct
type app struct {
	addr    string
	svr     *echo.Echo
	runtime *wails.Runtime

	// appMenu 不可视，主要用途热键
	appMenu *menu.Menu
	// 托盘菜单
	defaultTrayMenu *menu.TrayMenu
	//startsAtLoginMenu *menu.MenuItem
	//autoUpdateMenu *menu.MenuItem
	//appUpdatesMenu *menu.MenuItem

	Service *Service
	lock    sync.Mutex
	Verbose bool
	// support darkMode todo i'm not sure if windows can support dark mode.
	isDarkMode bool

	defaultTrayMenuActive bool

	// incomingURLSemaphore use channel that ensure only one event will be processed
	incomingURLSemaphore chan struct{}
}

// newApp creates a new app application struct
func newApp(addr string) (*app, error) {
	app := &app{
		addr:                 addr,
		incomingURLSemaphore: make(chan struct{}, ConcurrentIncomeURL),
	}
	app.appMenu = menu.NewMenuFromItems(
		menu.AppMenu(),
		menu.EditMenu(),
		menu.WindowMenu(),
	)

	// initialize service,
	// we put logic that interact with frontend into service
	app.Service = NewService()

	// 自动更新
	//app.appUpdatesMenu = &menu.MenuItem{
	//	Type:  menu.TextType,
	//	Label: "Check for updates...",
	//}

	// 启动时自动更新
	//app.autoUpdateMenu = &menu.MenuItem{
	//	Type:  menu.CheckboxType,
	//	Label: "Update automatically",
	//}

	// 是否启动运行
	//app.startsAtLoginMenu = &menu.MenuItem{
	//	Type:    menu.CheckboxType,
	//	Label:   "Start at Login",
	//	Checked: false,
	//}
	//startsAtLogin, err := mac.StartsAtLogin()
	//if err != nil {
	//	if app.Verbose {
	//		log.Println("start at login:", err)
	//	}
	//	app.startsAtLoginMenu.Label = "Start at Login"
	//	app.startsAtLoginMenu.Checked = true
	//} else {
	//	app.startsAtLoginMenu.Checked = startsAtLogin
	//}

	app.defaultTrayMenu = &menu.TrayMenu{
		Label: DaemonName,
		Menu:  app.newTrayMenu(),
	}

	return app, nil
}

func (b *app) startupServer(runtime *wails.Runtime) {
	log.Print("Startuup server????")

	svr := NewSvr()
	log.Print("start regiter routes I hope????")
	svr.RegisterRoutes()
	log.Print("done regiter routes????")
	err := svr.Start(runtime, b.addr)
	if err != nil {
		println(err.Error())
		os.Exit(1)
		return
	}
}

// startup is called at application startup
func (b *app) startup(runtime *wails.Runtime) {
	b.setDarkMode(runtime.System.IsDarkMode())
	// 暗黑模式更新
	runtime.Events.OnThemeChange(func(darkMode bool) {
		// keep track of dark mode changing, and refresh all
		// plugins if it does.
		b.setDarkMode(darkMode)
		b.refreshAll()
	})
	b.runtime = runtime
	b.Service.runtime = runtime
	b.refreshAll()

	log.Print("Calling startupserver ????")

	// export web service
	go b.startupServer(runtime)
	// 准备自动更新
	go func() {
		// 等待检查更新
		time.Sleep(10 * time.Second)
		for {
			// todo app.checkForUpdates(true)
			// 12小时等待再次检查更新
			time.Sleep(12 * time.Hour)
		}
	}()
}

func (b *app) refreshAll() {
	b.lock.Lock()
	defer b.lock.Unlock()
	if b.defaultTrayMenuActive {
		// only default menu - remove it
		b.runtime.Menu.DeleteTrayMenu(b.defaultTrayMenu)
		b.defaultTrayMenuActive = false
	}
	b.runtime.Menu.SetTrayMenu(b.defaultTrayMenu)
	b.defaultTrayMenuActive = true
	return
}

// shutdown is called at application termination
func (b *app) shutdown() {
	// Perform your teardown here
}

func (b *app) newTrayMenu() *menu.Menu {
	var items []*menu.MenuItem
	items = append(items, &menu.MenuItem{
		Type:     menu.TextType,
		Label:    fmt.Sprintf("manta-singer (%s)", version),
		Disabled: true,
	})
	items = append(items, &menu.MenuItem{
		Type:  menu.TextType,
		Label: "Open signer",
		Click: func(_ *menu.CallbackData) {
			b.runtime.Window.Show()
		},
	})
	//items = append(items, b.appUpdatesMenu)
	//items = append(items, b.autoUpdateMenu)
	//items = append(items, b.startsAtLoginMenu)
	items = append(items, menu.Separator())
	items = append(items, &menu.MenuItem{
		Type:        menu.TextType,
		Label:       "Quit",
		Accelerator: keys.CmdOrCtrl("q"),
		Click:       b.onQuitMenuClicked,
	})
	m := &menu.Menu{Items: items}
	return m
}

func (b *app) onQuitMenuClicked(_ *menu.CallbackData) {
	b.runtime.Quit()
}

func (b *app) setDarkMode(darkMode bool) {
	b.lock.Lock()
	defer b.lock.Unlock()
	b.isDarkMode = darkMode
}

func (b *app) handleIncomeURL(url string) {
	b.incomingURLSemaphore <- struct{}{}
	defer func() {
		<-b.incomingURLSemaphore
	}()
	log.Println("incoming URL: handleIncomingURL", url)
	incomingURL, err := parseIncomingURL(url)
	if err != nil {
		_, err2 := b.runtime.Dialog.Message(&dialog.MessageDialog{
			Type:         dialog.ErrorDialog,
			Title:        "Invalid URL",
			Message:      err.Error(),
			Buttons:      []string{"OK"},
			CancelButton: "OK",
		})
		if err2 != nil {
			log.Println(err2)
			return
		}
		return
	}
	switch incomingURL.Action {
	case "show":
		b.runtime.Window.Show()
	default:
		log.Printf("incoming URL: skipping, unknown action %q\n", incomingURL.Action)
	}
}
