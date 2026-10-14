import { BookText, Info, Keyboard, Settings2, SlidersHorizontal, TextCursorInput } from "lucide-react";
import { NavLink, useLocation } from "react-router";
import {
    Sidebar, SidebarContent, SidebarFooter, SidebarGroup, SidebarGroupContent,
    SidebarGroupLabel, SidebarMenu, SidebarMenuButton, SidebarMenuItem, useSidebar,
} from "@/components/ui/sidebar";
import { useUpdates } from "@/components/update-provider";

export const settingsPages = [
    { title: "入力と文字", url: "/", icon: TextCursorInput },
    { title: "変換と候補", url: "/conversion", icon: SlidersHorizontal },
    { title: "キーとローマ字", url: "/keys", icon: Keyboard },
    { title: "ユーザ辞書", url: "/dictionary", icon: BookText },
    { title: "詳細設定", url: "/advanced", icon: Settings2 },
    { title: "Azookeyについて", url: "/about", icon: Info },
];

export function AppSidebar() {
    const { pathname } = useLocation();
    const { setOpenMobile } = useSidebar();
    const { result } = useUpdates();
    const menuItem = (item: typeof settingsPages[number]) => (
        <SidebarMenuItem key={item.url}>
            <SidebarMenuButton asChild isActive={pathname === item.url} tooltip={item.title}>
                <NavLink to={item.url} end aria-label={item.url === "/about" && result?.update_available ? `${item.title}（更新があります）` : item.title} onClick={() => setOpenMobile(false)}>
                    <item.icon />
                    <span className="min-w-0 flex-1 truncate group-data-[collapsible=icon]:hidden">{item.title}</span>
                    {item.url === "/about" && result?.update_available && (
                        <span className="ml-auto shrink-0 rounded-full bg-red-600 px-2 py-0.5 text-[10px] font-medium text-white group-data-[collapsible=icon]:absolute group-data-[collapsible=icon]:right-1 group-data-[collapsible=icon]:top-1 group-data-[collapsible=icon]:size-2 group-data-[collapsible=icon]:p-0 group-data-[collapsible=icon]:text-[0px]" aria-label="更新があります">更新</span>
                    )}
                </NavLink>
            </SidebarMenuButton>
        </SidebarMenuItem>
    );
    return (
        <Sidebar collapsible="icon">
            <SidebarContent>
                <SidebarGroup>
                    <SidebarGroupLabel className="mb-3 h-auto px-2 py-3 text-sm font-semibold text-sidebar-foreground group-data-[collapsible=icon]:hidden">azooKey 設定</SidebarGroupLabel>
                    <SidebarGroupContent>
                        <SidebarMenu>{settingsPages.slice(0, -1).map(menuItem)}</SidebarMenu>
                    </SidebarGroupContent>
                </SidebarGroup>
            </SidebarContent>
            <SidebarFooter>
                <SidebarMenu>{menuItem(settingsPages[settingsPages.length - 1])}</SidebarMenu>
            </SidebarFooter>
        </Sidebar>
    );
}
